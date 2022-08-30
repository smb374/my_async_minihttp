use std::{borrow::Cow, mem::MaybeUninit, slice};

use bytes::{Bytes, BytesMut};
use httparse::{Status, EMPTY_HEADER};
use log::error;

type DataRange = (usize, usize);

/// A struct that represents an HTTP Request.
pub struct Request {
    method: DataRange,
    path: DataRange,
    version: u8,
    headers: [(DataRange, DataRange); 256],
    data: Bytes,
    body: Option<Bytes>,
    pub(crate) header_len: usize,
    pub(crate) body_len: usize,
}

/// An iterator that iterates over the headers of a request.
pub struct RequestHeaders<'req> {
    headers: slice::Iter<'req, (DataRange, DataRange)>,
    req: &'req Request,
}

impl Request {
    /// Returns the method (e.g. `GET`) of this request.
    pub fn method(&self) -> Cow<'_, str> {
        String::from_utf8_lossy(self.slice(&self.method))
    }
    /// Returns the method (e.g. `/`) of this request.
    pub fn path(&self) -> Cow<'_, str> {
        String::from_utf8_lossy(self.slice(&self.path))
    }
    /// Returns the method (e.g. `1`) of this request.
    pub fn version(&self) -> u8 {
        self.version
    }
    /// Returns an iterator that iterates the headers for further processing.
    pub fn headers(&self) -> RequestHeaders {
        RequestHeaders {
            headers: self.headers[..self.header_len].iter(),
            req: self,
        }
    }
    /// Returns the payload that comes with the request (e.g. `POST` data).
    pub fn body(&self) -> Option<&Bytes> {
        self.body.as_ref()
    }
    pub(crate) fn set_body(&mut self, body: Bytes) {
        self.body = Some(body);
    }
    pub(crate) fn slice(&self, data_range: &DataRange) -> &[u8] {
        &self.data[data_range.0..data_range.1]
    }
}

pub fn decode(buf: &mut BytesMut) -> Result<Option<Request>, httparse::Error> {
    let mut headers = [EMPTY_HEADER; 256];
    let mut req = httparse::Request::new(&mut headers);
    let mut body_len = 0;
    let status = req.parse(buf)?;
    let amt = match status {
        Status::Complete(amt) => amt,
        Status::Partial => return Ok(None),
    };
    let mut headers: [(DataRange, DataRange); 256] = unsafe {
        let h: [MaybeUninit<(DataRange, DataRange)>; 256] = MaybeUninit::uninit().assume_init();
        std::mem::transmute(h)
    };
    for (idx, h) in req.headers.iter().enumerate() {
        let name = h.name;
        let val = h.value;
        if name == "Content-Length" {
            body_len =
                usize::from_str_radix(&String::from_utf8_lossy(val), 10).unwrap_or_else(|e| {
                    error!("Failed to parse Content-Length into integer: {}", e);
                    0
                });
        }
        headers[idx] = (to_data_range(name.as_bytes(), buf), to_data_range(val, buf));
    }
    let header_len = req.headers.len();
    Ok(Some(Request {
        method: to_data_range(req.method.unwrap().as_bytes(), buf),
        path: to_data_range(req.path.unwrap().as_bytes(), buf),
        version: req.version.unwrap(),
        headers,
        data: buf.split_to(amt).freeze(),
        body: None,
        body_len,
        header_len,
    }))
}

fn to_data_range(s: &[u8], origin: &BytesMut) -> DataRange {
    let start = s.as_ptr() as usize - origin.as_ptr() as usize;
    debug_assert!(start < origin.len());
    (start, start + s.len())
}

impl<'req> Iterator for RequestHeaders<'req> {
    type Item = (Cow<'req, str>, &'req [u8]);
    fn next(&mut self) -> Option<Self::Item> {
        self.headers.next().map(|(a, b)| {
            let a = self.req.slice(a);
            let b = self.req.slice(b);
            (String::from_utf8_lossy(a), b)
        })
    }
}
