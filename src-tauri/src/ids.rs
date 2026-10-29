//! Names of owners and groups. SFTP v3 only reports numeric ids: a remote
//! session reads `/etc/passwd` and `/etc/group` once (unreadable, e.g. in a
//! chroot, or users from LDAP: the UI shows the numbers). Locally the
//! system's user database answers, through a small cache.

use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Debug, Default)]
pub struct IdNames {
    pub users: HashMap<u32, String>,
    pub groups: HashMap<u32, String>,
}

impl IdNames {
    /// `passwd` and `group` file contents: `name:x:id:...` lines.
    pub fn parse(passwd: &str, group: &str) -> Self {
        Self {
            users: parse_db(passwd),
            groups: parse_db(group),
        }
    }
}

fn parse_db(text: &str) -> HashMap<u32, String> {
    let mut map = HashMap::new();
    for line in text.lines().filter(|l| !l.starts_with('#')) {
        let mut fields = line.split(':');
        let (Some(name), Some(_), Some(id)) = (fields.next(), fields.next(), fields.next()) else { continue };
        if let Ok(id) = id.parse() {
            // The first entry wins, as for getpwuid.
            map.entry(id).or_insert_with(|| name.to_string());
        }
    }
    map
}

static LOCAL_USERS: Mutex<Option<HashMap<u32, Option<String>>>> = Mutex::new(None);
static LOCAL_GROUPS: Mutex<Option<HashMap<u32, Option<String>>>> = Mutex::new(None);

pub fn local_user(uid: u32) -> Option<String> {
    cached(&LOCAL_USERS, uid, |id| unsafe {
        let pw = libc::getpwuid(id);
        (!pw.is_null()).then(|| std::ffi::CStr::from_ptr((*pw).pw_name).to_string_lossy().into_owned())
    })
}

pub fn local_group(gid: u32) -> Option<String> {
    cached(&LOCAL_GROUPS, gid, |id| unsafe {
        let gr = libc::getgrgid(id);
        (!gr.is_null()).then(|| std::ffi::CStr::from_ptr((*gr).gr_name).to_string_lossy().into_owned())
    })
}

/// The lookup runs under the cache lock: getpwuid/getgrgid return static
/// buffers, so calls must not overlap.
fn cached(cache: &Mutex<Option<HashMap<u32, Option<String>>>>, id: u32, lookup: impl Fn(u32) -> Option<String>) -> Option<String> {
    let mut guard = cache.lock().unwrap();
    guard.get_or_insert_with(HashMap::new).entry(id).or_insert_with(|| lookup(id)).clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_passwd_and_group() {
        let names = IdNames::parse(
            "# comment\nroot:x:0:0:root:/root:/bin/bash\nwww-data:x:33:33::/var/www:/usr/sbin/nologin\nbroken\ntoor:x:0:0::/:/bin/sh\n",
            "root:x:0:\nwww-data:x:33:deploy\n",
        );
        assert_eq!(names.users.get(&0).map(String::as_str), Some("root"));
        assert_eq!(names.users.get(&33).map(String::as_str), Some("www-data"));
        assert_eq!(names.groups.get(&33).map(String::as_str), Some("www-data"));
        assert_eq!(names.users.len(), 2);
    }

    #[test]
    fn resolves_local_ids() {
        assert_eq!(local_user(0).as_deref(), Some("root"));
        assert!(local_group(0).is_some());
        let me = unsafe { libc::getuid() };
        assert_eq!(local_user(me), std::env::var("USER").ok());
    }
}
