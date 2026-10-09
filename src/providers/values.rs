//! Resolving `vars` for custom providers (environment variables, files,
//! Windows Credential Manager entries, command output) and JWT expiry.
//! Values are read when a request is made and never stored or logged.

use serde_json::Value;

use super::command;
use crate::config::ValueSource;
use crate::jpath;

pub fn resolve(src: &ValueSource) -> Result<String, String> {
    match src {
        ValueSource::Literal(s) => Ok(s.clone()),
        ValueSource::Env(name) => {
            std::env::var(name).ok().filter(|v| !v.is_empty()).ok_or_else(|| format!("environment variable {name} is not set"))
        }
        ValueSource::File { path, json } => {
            let text = std::fs::read_to_string(path).map_err(|_| format!("{} not found", path.display()))?;
            let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
            match json {
                None => Ok(text.trim().to_owned()),
                Some(p) => {
                    let v: Value = serde_json::from_str(text).map_err(|_| format!("{} is not valid JSON", path.display()))?;
                    jpath::get(&v, p).and_then(jpath::text).ok_or_else(|| format!("{p} not found in {}", path.display()))
                }
            }
        }
        ValueSource::Credential(target) => credential(target),
        ValueSource::Command(line) => {
            let out = command::run(line, 30)?;
            let out = out.trim();
            if out.is_empty() { Err("the command printed nothing".into()) } else { Ok(out.to_owned()) }
        }
    }
}

/// The password of a generic credential, e.g. one saved with
/// `cmdkey /generic:limitbar/openrouter /user:me /pass:<key>`.
fn credential(target: &str) -> Result<String, String> {
    use windows_sys::Win32::Security::Credentials::{CRED_TYPE_GENERIC, CREDENTIALW, CredFree, CredReadW};
    let mut pcred: *mut CREDENTIALW = std::ptr::null_mut();
    let ok = unsafe { CredReadW(crate::wide(target).as_ptr(), CRED_TYPE_GENERIC, 0, &mut pcred) };
    if ok == 0 || pcred.is_null() {
        return Err(format!("no generic credential named \"{target}\" in Windows Credential Manager"));
    }
    let blob = unsafe {
        let c = &*pcred;
        let bytes = if c.CredentialBlob.is_null() {
            Vec::new()
        } else {
            std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize).to_vec()
        };
        CredFree(pcred as *const _);
        bytes
    };
    let text = decode_blob(&blob);
    if text.is_empty() { Err(format!("credential \"{target}\" has an empty password")) } else { Ok(text) }
}

/// cmdkey and the Credential Manager UI store UTF-16; other tools store UTF-8.
fn decode_blob(b: &[u8]) -> String {
    let utf16 = b.len().is_multiple_of(2) && b.len() >= 2 && b.chunks(2).filter(|c| c[1] == 0).count() * 2 >= b.len() / 2;
    let s = if utf16 {
        String::from_utf16_lossy(&b.chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect::<Vec<_>>())
    } else {
        String::from_utf8_lossy(b).into_owned()
    };
    s.trim_matches(['\0', ' ', '\r', '\n', '\t']).to_owned()
}

/// Unpadded or padded base64url (also accepts the standard alphabet).
fn base64url(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            b'=' => break,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// The `exp` claim (unix seconds) of a JWT, without verifying it.
pub fn jwt_exp(token: &str) -> Option<i64> {
    let token = token.strip_prefix("Bearer ").unwrap_or(token);
    let payload = base64url(token.split('.').nth(1)?)?;
    let claims: Value = serde_json::from_slice(&payload).ok()?;
    let exp = claims.get("exp")?;
    exp.as_i64().or_else(|| exp.as_f64().map(|f| f as i64))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CommandLine;

    #[test]
    fn jwt_expiry() {
        // header {"alg":"HS256","typ":"JWT"}, payload {"sub":"u","exp":1796716831,"type":"session"}
        let token = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ1IiwiZXhwIjoxNzk2NzE2ODMxLCJ0eXBlIjoic2Vzc2lvbiJ9.sig"; // gitleaks:allow (synthetic test token, signature "sig")
        assert_eq!(jwt_exp(token), Some(1_796_716_831));
        assert_eq!(jwt_exp("not-a-jwt"), None);
        assert_eq!(jwt_exp("a.@@@.c"), None);
        // payload {"sub":"u"} has no exp
        assert_eq!(jwt_exp("x.eyJzdWIiOiJ1In0.y"), None);
    }

    #[test]
    fn base64url_variants() {
        assert_eq!(base64url("aGk").unwrap(), b"hi");
        assert_eq!(base64url("aGk=").unwrap(), b"hi");
        assert_eq!(base64url("-_8").unwrap(), [0xfb, 0xff]);
    }

    #[test]
    fn blobs() {
        let utf16: Vec<u8> = "sk-123".encode_utf16().flat_map(u16::to_le_bytes).collect();
        assert_eq!(decode_blob(&utf16), "sk-123");
        assert_eq!(decode_blob(b"sk-abcd"), "sk-abcd");
        assert_eq!(decode_blob(b"ab"), "ab");
    }

    #[test]
    fn sources() {
        let dir = std::env::temp_dir().join(format!("limitbar-values-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("auth.json");
        std::fs::write(&f, "\u{feff}{\"tokens\":{\"access_token\":\"abc\",\"n\":5}}").unwrap();
        let file = |json: Option<&str>| ValueSource::File { path: f.clone(), json: json.map(str::to_owned) };
        assert_eq!(resolve(&file(Some("/tokens/access_token"))).unwrap(), "abc");
        assert_eq!(resolve(&file(Some("/tokens/n"))).unwrap(), "5");
        assert!(resolve(&file(Some("/nope"))).is_err());
        std::fs::write(dir.join("key.txt"), "  plain-key\r\n").unwrap();
        assert_eq!(resolve(&ValueSource::File { path: dir.join("key.txt"), json: None }).unwrap(), "plain-key");
        assert!(resolve(&ValueSource::File { path: dir.join("missing"), json: None }).is_err());
        unsafe { std::env::set_var("LIMITBAR_TEST_KEY", "k1") };
        assert_eq!(resolve(&ValueSource::Env("LIMITBAR_TEST_KEY".into())).unwrap(), "k1");
        assert!(resolve(&ValueSource::Env("LIMITBAR_TEST_UNSET_VAR".into())).is_err());
        assert_eq!(resolve(&ValueSource::Command(CommandLine::Shell("echo  tok-42 ".into()))).unwrap(), "tok-42");
        assert!(resolve(&ValueSource::Credential("limitbar-test/does-not-exist".into())).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }
}
