//! ICMP "sockets".

use core::future::poll_fn;
use core::mem;
use core::task::{Context, Poll};

use embassy_net_driver::Driver;
use smoltcp::iface::{Interface, SocketHandle};
use smoltcp::socket::icmp::{self, Endpoint, PacketMetadata};
use smoltcp::wire::IpAddress;

use crate::Stack;

/// Error returned by [`IcmpSocket::recv`] and [`IcmpSocket::send`].
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum RecvError {
    /// Provided buffer was smaller than the received packet.
    Exhausted,
    /// Provided received packet is not complete.
    Truncated,
}

/// An ICMP socket.
pub struct IcmpSocket<'a> {
    stack: Stack<'a>,
    handle: SocketHandle,
    endpoint: Endpoint
}

impl<'a> IcmpSocket<'a> {
    /// Create a new ICMP socket using the provided stack and buffers.
    pub fn new<D: Driver>(
        stack: Stack<'a>,
        endpoint: Endpoint,
        rx_meta: &'a mut [PacketMetadata],
        rx_buffer: &'a mut [u8],
        tx_meta: &'a mut [PacketMetadata],
        tx_buffer: &'a mut [u8],
    ) -> Self {
        let handle = stack.with_mut(|i| {
            let rx_meta: &'static mut [PacketMetadata] = unsafe { mem::transmute(rx_meta) };
            let rx_buffer: &'static mut [u8] = unsafe { mem::transmute(rx_buffer) };
            let tx_meta: &'static mut [PacketMetadata] = unsafe { mem::transmute(tx_meta) };
            let tx_buffer: &'static mut [u8] = unsafe { mem::transmute(tx_buffer) };
            i.sockets.add(icmp::Socket::new(
                icmp::PacketBuffer::new(rx_meta, rx_buffer),
                icmp::PacketBuffer::new(tx_meta, tx_buffer),
            ))
        });

        Self { stack, handle, endpoint }
    }

    fn with_mut<R>(&self, f: impl FnOnce(&mut icmp::Socket, &mut Interface) -> R) -> R {
        self.stack.with_mut(|i| {
            let socket = i.sockets.get_mut::<icmp::Socket>(self.handle);
            let res = f(socket, &mut i.iface);
            i.waker.wake();
            res
        })
    }

    /// Receive a datagram.
    ///
    /// This method will wait until a datagram is received.
    pub async fn recv(&self, buf: &mut [u8]) -> Result<(usize, IpAddress), RecvError> {
        poll_fn(move |cx| self.poll_recv(buf, cx)).await
    }

    /// Receive a datagram.
    ///
    /// When no datagram is available, this method will return `Poll::Pending` and
    /// register the current task to be notified when a datagram is received.
    pub fn poll_recv(&self, buf: &mut [u8], cx: &mut Context<'_>) -> Poll<Result<(usize, IpAddress), RecvError>> {
        self.with_mut(|s, _| match s.recv_slice(buf) {
            Ok(n) => Poll::Ready(Ok(n)),
            // No data ready
            Err(icmp::RecvError::Truncated) => Poll::Ready(Err(RecvError::Truncated)),
            Err(icmp::RecvError::Exhausted) => {
                s.register_recv_waker(cx.waker());
                Poll::Pending
            }
        })
    }

    /// Send a datagram.
    ///
    /// This method will wait until the datagram has been sent.`
    pub async fn send(&self, buf: &[u8]) {
        poll_fn(move |cx| self.poll_send(buf, cx)).await
    }

    /// Send a datagram.
    ///
    /// When the datagram has been sent, this method will return `Poll::Ready(Ok())`.
    ///
    /// When the socket's send buffer is full, this method will return `Poll::Pending`
    /// and register the current task to be notified when the buffer has space available.
    pub fn poll_send(&self, buf: &[u8], cx: &mut Context<'_>) -> Poll<()> {
        if !self.endpoint.is_specified() {
            return Poll::Pending; // TODO: definitely not
        }
        // where should this be sent, ident / vs option addr + port, ip v4/v6
        let dst = match self.endpoint {
            Endpoint::Unspecified => todo!(),
            Endpoint::Ident(_) => todo!(),
            Endpoint::Udp(listen_endpoint) => listen_endpoint.addr.expect("TODO - ident/none"),
        };

        self.with_mut(|s, _| match s.send_slice(buf, dst) {
            // Entire datagram has been sent
            Ok(()) => Poll::Ready(()),
            Err(icmp::SendError::BufferFull) => {
                s.register_send_waker(cx.waker());
                Poll::Pending
            }
            Err(icmp::SendError::Unaddressable) => {
                unimplemented!()
            }
        })
    }

    /// Flush the socket.
    ///
    /// This method will wait until the socket is flushed.
    pub async fn flush(&mut self) {
        poll_fn(move |cx| {
            self.with_mut(|s, _| {
                if s.send_queue() == 0 {
                    Poll::Ready(())
                } else {
                    s.register_send_waker(cx.waker());
                    Poll::Pending
                }
            })
        })
        .await
    }
}

impl Drop for IcmpSocket<'_> {
    fn drop(&mut self) {
        self.stack.with_mut(|i| i.sockets.remove(self.handle));
    }
}

fn _assert_covariant<'a, 'b: 'a>(x: IcmpSocket<'b>) -> IcmpSocket<'a> {
    x
}
