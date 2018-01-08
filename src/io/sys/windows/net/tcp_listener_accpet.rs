use std;
use std::io;
use std::net::SocketAddr;
use std::os::windows::io::AsRawSocket;
use winapi::shared::ntdef::*;
use scheduler::get_scheduler;
use io::cancel::CancelIoData;
use sync::delay_drop::DelayDrop;
use net::{TcpListener, TcpStream};
use miow::net::{AcceptAddrsBuf, TcpListenerExt};
use super::super::{add_socket, co_io_result, EventData};
use coroutine_impl::{co_cancel_data, CoroutineImpl, EventSource};

pub struct TcpListenerAccept<'a> {
    io_data: EventData,
    socket: &'a std::net::TcpListener,
    ret: std::net::TcpStream,
    addr: AcceptAddrsBuf,
    can_drop: DelayDrop,
}

impl<'a> TcpListenerAccept<'a> {
    pub fn new(socket: &'a TcpListener) -> io::Result<Self> {
        use socket2::{Domain, Socket, Type};

        let local_addr = socket.local_addr()?;
        let stream = match local_addr {
            SocketAddr::V4(..) => Socket::new(Domain::ipv4(), Type::stream(), None)?,
            SocketAddr::V6(..) => Socket::new(Domain::ipv4(), Type::stream(), None)?,
        };
        let stream = stream.into_tcp_stream();

        Ok(TcpListenerAccept {
            io_data: EventData::new(socket.as_raw_socket() as HANDLE),
            socket: socket.inner(),
            ret: stream,
            addr: AcceptAddrsBuf::new(),
            can_drop: DelayDrop::new(),
        })
    }

    #[inline]
    pub fn done(self) -> io::Result<(TcpStream, SocketAddr)> {
        try!(co_io_result(&self.io_data));
        let socket = &self.socket;
        let ss = self.ret;
        let s = try!(socket.accept_complete(&ss).and_then(|_| {
            try!(ss.set_nonblocking(true));
            add_socket(&ss).map(|io| TcpStream::from_stream(ss, io))
        }));

        let addr = try!(
            self.addr
                .parse(&self.socket)
                .and_then(|a| a.remote().ok_or_else(|| io::Error::new(
                    io::ErrorKind::Other,
                    "could not obtain remote address"
                )))
        );

        Ok((s, addr))
    }
}

impl<'a> EventSource for TcpListenerAccept<'a> {
    fn subscribe(&mut self, co: CoroutineImpl) {
        let _g = self.can_drop.delay_drop();
        let s = get_scheduler();
        let cancel = co_cancel_data(&co);
        // we don't need to register the timeout here,
        // prepare the co first
        self.io_data.co = Some(co);

        // call the overlapped read API
        co_try!(s, self.io_data.co.take().expect("can't get co"), unsafe {
            self.socket
                .accept_overlapped(&self.ret, &mut self.addr, self.io_data.get_overlapped())
        });

        // register the cancel io data
        cancel.set_io(CancelIoData::new(&self.io_data));
        // re-check the cancel status
        if cancel.is_canceled() {
            unsafe { cancel.cancel() };
        }
    }
}
