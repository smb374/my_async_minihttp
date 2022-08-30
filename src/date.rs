use std::{
    borrow::Cow,
    cell::RefCell,
    fmt::{self, Write},
    str,
};

use chrono::{DateTime, Duration, Local};

pub struct Now;

struct CachedNow {
    bytes: [u8; 128],
    amt: usize,
    next_update: Option<DateTime<Local>>,
}

struct TimeBuffer<'a>(&'a mut CachedNow);

impl<'a> fmt::Write for TimeBuffer<'a> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let start = self.0.amt;
        let end = start + s.len();
        self.0.bytes[start..end].copy_from_slice(s.as_bytes());
        self.0.amt += s.len();
        Ok(())
    }
}
pub fn now() -> Now {
    Now
}

thread_local! {
    static LAST: RefCell<CachedNow> = RefCell::new(CachedNow { bytes: [0u8; 128], amt: 0, next_update: None });
}

impl fmt::Display for Now {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        LAST.with(|c| {
            let mut cache = c.borrow_mut();
            let now = Local::now();
            match cache.next_update {
                Some(nu) => {
                    if now > nu {
                        cache.update(now);
                    }
                }
                None => cache.update(now),
            }
            f.write_str(&cache.buffer())
        })
    }
}

impl CachedNow {
    fn buffer(&self) -> Cow<'_, str> {
        String::from_utf8_lossy(&self.bytes[..self.amt])
    }
    fn update(&mut self, now: DateTime<Local>) {
        self.amt = 0;
        write!(TimeBuffer(self), "{}", now.to_rfc2822()).unwrap();
        self.next_update.replace(now + Duration::seconds(1));
    }
}
