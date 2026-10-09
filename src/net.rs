//! Minimal HTTP(S) client over WinHTTP (system TLS, system proxy, no extra crates).

use std::ffi::c_void;
use std::ptr::null;

use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::Networking::WinHttp::*;

use crate::wide;

pub struct Response {
    pub status: u32,
    pub body: Vec<u8>,
    pub retry_after: Option<u32>,
}

pub struct Http {
    session: *mut c_void,
}

// WinHTTP session handles may be shared between threads; every request opens
// its own connection and request handles.
unsafe impl Send for Http {}
unsafe impl Sync for Http {}

const MAX_BODY: usize = 4 << 20;

struct Handle(*mut c_void);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { WinHttpCloseHandle(self.0) };
        }
    }
}

fn last_error(what: &str) -> String {
    let code = unsafe { GetLastError() };
    let reason = match code {
        12002 => "timed out",
        12005 => "invalid URL",
        12006 => "unrecognised URL scheme",
        12007 => "host not found (offline?)",
        12029 | 12030 => "connection failed (offline?)",
        12175 => "TLS error",
        _ => "",
    };
    if reason.is_empty() { format!("{what} failed (error {code})") } else { format!("{what}: {reason}") }
}

/// The parts of an `http://` or `https://` URL WinHTTP needs.
#[derive(Debug, PartialEq)]
pub struct Url<'a> {
    pub secure: bool,
    pub host: &'a str,
    pub port: u16,
    /// Path and query, starting with `/`.
    pub path: String,
}

pub fn parse_url(url: &str) -> Result<Url<'_>, String> {
    let (secure, rest) = if let Some(r) = url.strip_prefix("https://") {
        (true, r)
    } else if let Some(r) = url.strip_prefix("http://") {
        (false, r)
    } else {
        return Err(format!("URL must start with https:// or http://: {url}"));
    };
    let split = rest.find(['/', '?']).unwrap_or(rest.len());
    let (authority, path) = rest.split_at(split);
    let path = if path.starts_with('/') { path.to_owned() } else { format!("/{path}") };
    let bad_port = || format!("bad port in URL: {url}");
    let (host, port) = if let Some(v6) = authority.strip_prefix('[') {
        let (h, tail) = v6.split_once(']').ok_or_else(|| format!("bad host in URL: {url}"))?;
        match tail.strip_prefix(':') {
            Some(p) => (h, p.parse::<u16>().map_err(|_| bad_port())?),
            None if tail.is_empty() => (h, 0),
            None => return Err(bad_port()),
        }
    } else {
        match authority.rsplit_once(':') {
            Some((h, p)) => (h, p.parse::<u16>().map_err(|_| bad_port())?),
            None => (authority, 0),
        }
    };
    if host.is_empty() || host.contains('@') {
        return Err(format!("bad host in URL: {url}"));
    }
    let port = if port == 0 { if secure { 443 } else { 80 } } else { port };
    Ok(Url { secure, host, port, path })
}

impl Http {
    pub fn new(user_agent: &str) -> Option<Self> {
        unsafe {
            let session = WinHttpOpen(wide(user_agent).as_ptr(), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, null(), null(), 0);
            if session.is_null() {
                return None;
            }
            WinHttpSetTimeouts(session, 10_000, 10_000, 15_000, 15_000);
            let flags: u32 = WINHTTP_DECOMPRESSION_FLAG_GZIP | WINHTTP_DECOMPRESSION_FLAG_DEFLATE;
            WinHttpSetOption(session, WINHTTP_OPTION_DECOMPRESSION, &flags as *const u32 as *const c_void, 4);
            Some(Http { session })
        }
    }

    pub fn get(&self, host: &str, path: &str, headers: &[(&str, &str)]) -> Result<Response, String> {
        self.request("GET", &Url { secure: true, host, port: 443, path: path.to_owned() }, headers, &[])
    }

    pub fn post(&self, host: &str, path: &str, headers: &[(&str, &str)], body: &[u8]) -> Result<Response, String> {
        self.request("POST", &Url { secure: true, host, port: 443, path: path.to_owned() }, headers, body)
    }

    pub fn request(&self, method: &str, url: &Url, headers: &[(&str, &str)], body: &[u8]) -> Result<Response, String> {
        unsafe {
            let conn = Handle(WinHttpConnect(self.session, wide(url.host).as_ptr(), url.port, 0));
            if conn.0.is_null() {
                return Err(last_error("connect"));
            }
            let req = Handle(WinHttpOpenRequest(
                conn.0,
                wide(method).as_ptr(),
                wide(&url.path).as_ptr(),
                null(),
                null(),
                null(),
                if url.secure { WINHTTP_FLAG_SECURE } else { 0 },
            ));
            if req.0.is_null() {
                return Err(last_error("request"));
            }
            let mut hdr = String::new();
            for (k, v) in headers {
                hdr.push_str(k);
                hdr.push_str(": ");
                hdr.push_str(v);
                hdr.push_str("\r\n");
            }
            let hdr_w: Vec<u16> = hdr.encode_utf16().collect();
            let (data, len) = if body.is_empty() { (null(), 0) } else { (body.as_ptr() as *const c_void, body.len() as u32) };
            if WinHttpSendRequest(req.0, hdr_w.as_ptr(), hdr_w.len() as u32, data, len, len, 0) == 0 {
                return Err(last_error("send"));
            }
            if WinHttpReceiveResponse(req.0, std::ptr::null_mut()) == 0 {
                return Err(last_error("receive"));
            }
            let status = query_number(req.0, WINHTTP_QUERY_STATUS_CODE).unwrap_or(0);
            let retry_after = query_number(req.0, WINHTTP_QUERY_RETRY_AFTER);
            let mut body = Vec::new();
            loop {
                let mut avail = 0u32;
                if WinHttpQueryDataAvailable(req.0, &mut avail) == 0 {
                    return Err(last_error("read"));
                }
                if avail == 0 {
                    break;
                }
                let start = body.len();
                body.resize(start + avail as usize, 0);
                let mut read = 0u32;
                if WinHttpReadData(req.0, body[start..].as_mut_ptr() as *mut c_void, avail, &mut read) == 0 {
                    return Err(last_error("read"));
                }
                body.truncate(start + read as usize);
                if read == 0 || body.len() > MAX_BODY {
                    break;
                }
            }
            Ok(Response { status, body, retry_after })
        }
    }
}

unsafe fn query_number(req: *mut c_void, what: u32) -> Option<u32> {
    let mut value = 0u32;
    let mut size = 4u32;
    let ok = unsafe {
        WinHttpQueryHeaders(
            req,
            what | WINHTTP_QUERY_FLAG_NUMBER,
            null(),
            &mut value as *mut u32 as *mut c_void,
            &mut size,
            std::ptr::null_mut(),
        )
    };
    (ok != 0).then_some(value)
}

impl Drop for Http {
    fn drop(&mut self) {
        unsafe { WinHttpCloseHandle(self.session) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        let u = parse_url("https://openrouter.ai/api/v1/credits?x=1").unwrap();
        assert_eq!(u, Url { secure: true, host: "openrouter.ai", port: 443, path: "/api/v1/credits?x=1".into() });
        let u = parse_url("http://localhost:4000").unwrap();
        assert_eq!(u, Url { secure: false, host: "localhost", port: 4000, path: "/".into() });
        let u = parse_url("https://example.com?q").unwrap();
        assert_eq!((u.host, u.path.as_str()), ("example.com", "/?q"));
        let u = parse_url("http://[::1]:8080/x").unwrap();
        assert_eq!((u.host, u.port, u.path.as_str()), ("::1", 8080, "/x"));
        assert_eq!(parse_url("https://[::1]/").unwrap().port, 443);
        assert!(parse_url("ftp://x").is_err());
        assert!(parse_url("https://:80/").is_err());
        assert!(parse_url("https://host:abc/").is_err());
        assert!(parse_url("https://user@host/").is_err());
    }
}
