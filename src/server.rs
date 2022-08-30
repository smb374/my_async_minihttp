use crate::{
    request::{self, Request},
    response::Response,
};

use std::{io, net::ToSocketAddrs};

use async_trait::async_trait;
use bytes::{Buf, BufMut, BytesMut};
use futures_lite::{AsyncReadExt, AsyncWriteExt};
use log::{error, info};
use my_async::{
    multi_thread::spawn,
    net::{TcpListener, TcpStream},
    schedulers::JoinHandle,
};

const BYTES_POOL_SIZE: usize = 4096 * 128;

/// A trait that defines the behaviour of a service.
///
/// **Note**: Anything implements this should add `#[async_trait]` attribute.
///
/// [`macro@async_trait`] is re-exported in the top-level for convenience.
#[async_trait]
pub trait HttpService {
    /// Defines the behavious of the service by implementing this function.
    ///
    /// Do what ever you like in this function, but remember to write `response` before it
    /// returns.
    async fn call(&mut self, request: Request, response: &mut Response) -> io::Result<()>;
}

/// A factory used to define a server which process a request with a [`Service`][a] and write back
/// responses.
///
/// You should only need to define [`Service`][a] &
/// [`new_service()`][b] to serve pages.
///
/// You can run the service with [`start`][c] with an address to bind or
/// [`start_with_listener`][d] with a supplied [`TcpListener`][e]
///
/// [a]: HttpServiceFactory::Service
/// [b]: HttpServiceFactory::new_service()
/// [c]: HttpServiceFactory::start()
/// [d]: HttpServiceFactory::start_with_listener()
/// [e]: my_async::net::TcpListener
pub trait HttpServiceFactory: Send + Sized + 'static {
    /// The service type that [`new_service()`][a] emits.
    ///
    /// [a]: HttpServiceFactory::new_service()
    type Service: HttpService + Send;

    /// The function that return the service you've defined.
    fn new_service(&self) -> Self::Service;
    /// Starts the service with an address to bind.
    ///
    /// uses the same address.
    fn start<A: ToSocketAddrs>(self, addr: A) -> io::Result<JoinHandle<io::Result<()>>> {
        let handle = self.start_with_listener(TcpListener::bind(addr)?)?;
        Ok(handle)
    }
    /// Starts the service with a provided [`TcpListener`][a]
    ///
    /// You should use this method if you want to configure the [`TcpListener`][a] used by the
    /// server.
    ///
    /// [a]: my_async::net::TcpListener
    fn start_with_listener(self, listener: TcpListener) -> io::Result<JoinHandle<io::Result<()>>> {
        // spawns acceptor future for the factory.
        let handle = spawn(async move {
            let mut handles: Vec<JoinHandle<io::Result<()>>> = Vec::with_capacity(1024);
            loop {
                match listener.accept().await {
                    Ok((stream, addr)) => {
                        info!("Received connection by: {}", addr);
                        let service = self.new_service();
                        // spawns handler for each connection.
                        let h = spawn(handler(stream, service));
                        handles.push(h);
                    }
                    Err(e) => {
                        // Join all futures spawned before exit with error.
                        while !handles.is_empty() {
                            handles.retain(|h| match h.try_join() {
                                Some(result) => {
                                    if let Err(e) = result {
                                        error!("Error handling connection: {}", e);
                                    }
                                    false
                                }
                                None => true,
                            });
                        }
                        break Err(e);
                    }
                }
            }
        });
        Ok(handle)
    }
}

/// An HTTP Server that holds one service.
///
/// **Note**: This type implements [`HttpServiceFactory`] by simply cloning the underlying Service
/// type `T`.
///
/// If you want a more complex control over the service that emits by [`new_serivce`][a], consider
/// implementing [`HttpServiceFactory`]
///
/// [a]: HttpServiceFactory::new_service()
pub struct HttpServer<T>(pub T);

fn internal_error_resp<'a>(e: io::Error) -> Response<'a> {
    error!("error in service: {}", e);
    let mut resp = Response::new();
    resp.status_code(500, &"Internal Server Error")
        .body(e.to_string());
    resp
}

async fn handler<T: HttpService + Send>(mut stream: TcpStream, service: T) -> io::Result<()> {
    let mut bytes_pool: BytesMut = BytesMut::with_capacity(BYTES_POOL_SIZE);
    let mut req_bytes_cnt: usize = 0;
    // state for reading body.
    let mut reading_body = false;
    let mut remain_body_len: Option<usize> = None;
    let mut req_slot: Option<Request> = None;
    // read request
    loop {
        let buf = get_slice(&mut bytes_pool);
        match stream.read(buf).await {
            Ok(0) => return Ok(()),
            Ok(n) => {
                req_bytes_cnt += n;
                if let Some(k) = remain_body_len.as_mut() {
                    *k -= n;
                }
                advance(&mut bytes_pool, n);

                // section end mark, which is a blank line
                let section_end =
                    &bytes_pool[req_bytes_cnt - 4..req_bytes_cnt] == &[13, 10, 13, 10];
                if section_end {
                    if !reading_body {
                        match request::decode(&mut bytes_pool) {
                            Ok(Some(mut req)) => {
                                if req.body_len > 0 {
                                    reading_body = true;
                                    let remain = req.body_len - bytes_pool.len();
                                    // check if we've already read the body.
                                    if remain == 0 {
                                        let body = bytes_pool.split_to(req.body_len).freeze();
                                        req.set_body(body);
                                        break process_and_write_response(stream, service, req)
                                            .await;
                                    } else {
                                        // store states.
                                        remain_body_len = Some(remain);
                                        req_bytes_cnt = bytes_pool.len();
                                        req_slot = Some(req);
                                        continue;
                                    }
                                } else {
                                    break process_and_write_response(stream, service, req).await;
                                }
                            }
                            Ok(None) => {
                                error!("Request should be completed but resolved as a partial request! Quit connection...");
                                return Ok(());
                            }
                            Err(e) => {
                                error!("Request parse error: {}", e);
                                return Ok(());
                            }
                        }
                    } else {
                        if let Some(0) = remain_body_len {
                            let mut req = req_slot.take().unwrap();
                            let body = bytes_pool.split_to(req.body_len).freeze();
                            req.set_body(body);
                            break process_and_write_response(stream, service, req).await;
                        }
                    }
                }
            }
            Err(e) => break Err(e),
        }
    }
}

async fn process_and_write_response<T: HttpService + Send>(
    mut stream: TcpStream,
    mut service: T,
    req: Request,
) -> io::Result<()> {
    let mut resp = Response::new();
    let mut resp_bytes = if let Err(e) = service.call(req, &mut resp).await {
        internal_error_resp(e).encode()
    } else {
        resp.encode()
    };
    let mut left = resp_bytes.len();
    while left > 0 {
        match stream.write(&resp_bytes).await {
            Ok(0) => break,
            Ok(n) => {
                left -= n;
                resp_bytes.advance(n);
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

fn get_slice(bytes: &mut BytesMut) -> &mut [u8] {
    let remaining = bytes.capacity() - bytes.len();
    if remaining < 512 {
        bytes.reserve(BYTES_POOL_SIZE - remaining);
    }
    let buf = bytes.chunk_mut();
    unsafe { std::slice::from_raw_parts_mut(buf.as_mut_ptr(), buf.len()) }
}

fn advance(bytes: &mut BytesMut, len: usize) {
    unsafe {
        bytes.advance_mut(len);
    }
}

impl<T> HttpServiceFactory for HttpServer<T>
where
    T: HttpService + Send + Sync + Clone + 'static,
{
    type Service = T;
    fn new_service(&self) -> Self::Service {
        self.0.clone()
    }
}
