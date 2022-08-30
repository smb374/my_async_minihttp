use std::{
    borrow::Cow,
    fmt::{self, Write},
};

use bytes::{BufMut, Bytes, BytesMut};
use httparse::Header;

pub struct Response<'a> {
    headers: Vec<Header<'a>>,
    response: BytesMut,
    status_msg: StatusMsg<'a>,
}

enum StatusMsg<'a> {
    Ok,
    Custom(u32, Cow<'a, str>),
}

struct FastWrite<'a>(&'a mut BytesMut);

impl<'a> Response<'a> {
    pub fn new() -> Self {
        Self {
            headers: Vec::with_capacity(256),
            response: BytesMut::with_capacity(4096),
            status_msg: StatusMsg::Ok,
        }
    }
    pub fn status_code<T: AsRef<str>>(&mut self, code: u32, msg: &'a T) -> &mut Self {
        self.status_msg = StatusMsg::Custom(code, Cow::from(msg.as_ref()));
        self
    }
    pub fn header<T: AsRef<str>, U: AsRef<str>>(&mut self, name: &'a T, val: &'a U) -> &mut Self {
        self.headers.push(Header {
            name: name.as_ref(),
            value: val.as_ref().as_bytes(),
        });
        self
    }
    pub fn body<T: AsRef<str>>(&mut self, s: T) -> &mut Self {
        self.response.clear();
        self.response.put(s.as_ref().as_bytes());
        self
    }
    pub fn body_bytes<T: AsRef<[u8]>>(&mut self, s: T) -> &mut Self {
        self.response.clear();
        self.response.put(s.as_ref());
        self
    }
    pub(crate) fn encode(&self) -> Bytes {
        let mut buf = BytesMut::with_capacity(4096);
        let length = self.response.len();
        let now = crate::date::now();
        write!(
            FastWrite(&mut buf),
            "HTTP/1.1 {}\r\nServer: Example\r\nContent-Length: {}\r\nDate: {}\r\n",
            self.status_msg,
            length,
            now
        )
        .unwrap();
        self.headers.iter().for_each(|h| {
            buf.put(h.name.as_bytes());
            buf.put(&b": "[..]);
            buf.put(h.value);
            buf.put(&b"\r\n"[..]);
        });
        buf.put("\r\n".as_bytes());
        buf.put(self.response.as_ref());
        buf.freeze()
    }
}

impl<'a> fmt::Write for FastWrite<'a> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.0.put(s.as_bytes());
        Ok(())
    }
}

impl<'a> fmt::Display for StatusMsg<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StatusMsg::Ok => f.pad("200 OK"),
            StatusMsg::Custom(c, msg) => write!(f, "{} {}", c, msg),
        }
    }
}
