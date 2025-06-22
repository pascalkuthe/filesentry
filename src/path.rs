use std::cmp::Ordering;
use std::ffi::{CStr, OsStr};
use std::fmt::{Debug, Display};
use std::hash::{Hash, Hasher};
use std::mem::transmute;
use std::ops::Deref;
use std::path::Path;
use std::slice;

#[cfg(unix)]
const PATH_SEPERATOR: u8 = b'/';
#[cfg(windows)]
const PATH_SEPERATOR: u8 = b'\\';

use ecow::EcoVec;
use memchr::memrchr;

#[repr(transparent)]
#[derive(PartialEq, Eq)]
pub struct CannonicalPath {
    bytes: [u8],
}

#[cfg(unix)]
const EMPTY: &[u8] = b".\0";
#[cfg(windows)]
const EMPTY: &[u8] = b".";

impl CannonicalPath {
    pub fn as_std_path(&self) -> &Path {
        // safety: type ensures that self.buf is composition of
        // OsStr (and str but every str is an OsStr) and therefore always
        // valid
        Path::new(self.as_os_str())
    }

    pub fn parent(&self) -> Option<&Path> {
        let i = memrchr(PATH_SEPERATOR, &self.bytes)?;
        // safety: type ensures that self.buf is composition of
        // OsStr (and str but every str is an OsStr) and therefore always
        // valid
        let path = unsafe { OsStr::from_encoded_bytes_unchecked(&self.bytes[..i]) };
        Some(Path::new(path))
    }

    pub fn join(&self, other: &OsStr) -> CannonicalPathBuf {
        if self.is_empty() {
            let mut res = CannonicalPathBuf::new();
            res.push(other);
            res
        } else {
            let mut res = CannonicalPathBuf::with_capacity(self.bytes.len() + other.len());
            res.buf.extend_from_slice(&self.bytes);
            res.push(other);
            res
        }
    }

    fn as_raw_bytes(&self) -> &[u8] {
        if self.bytes.is_empty() {
            EMPTY
        } else {
            &self.bytes
        }
    }

    pub fn as_bytes(&self) -> &[u8] {
        let bytes = self.as_raw_bytes();
        if cfg!(unix) {
            &bytes[..bytes.len() - 1]
        } else {
            bytes
        }
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    pub fn len(&self) -> usize {
        if cfg!(unix) {
            self.bytes.len().saturating_sub(1)
        } else {
            self.bytes.len()
        }
    }

    pub fn as_os_str(&self) -> &OsStr {
        // safety: type ensures that self.buf is composition of
        // OsStr (and str but every str is an OsStr) and therefore always
        // valid
        unsafe { OsStr::from_encoded_bytes_unchecked(self.as_bytes()) }
    }

    #[cfg(unix)]
    pub fn as_c_str(&self) -> &CStr {
        // safety: type is always null terminated by construction
        unsafe { CStr::from_bytes_with_nul_unchecked(self.as_raw_bytes()) }
    }

    pub fn is_parent_of(&self, other: &CannonicalPath) -> bool {
        other.as_bytes().starts_with(self.as_bytes()) && other.bytes[self.len()] == PATH_SEPERATOR
    }
}

/// A custom PathBuf type that has some desirable properties:
///
/// * only 2 words size reducing memory pressure
/// * reference counted
/// * mutation via copy on write
/// * always cannocialized enabling fast bytewise comparsions
/// * nevers ends with a path seperator
#[derive(PartialEq, Eq, Clone)]
pub struct CannonicalPathBuf {
    buf: EcoVec<u8>,
}

impl PartialOrd for CannonicalPathBuf {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for CannonicalPathBuf {
    fn cmp(&self, other: &Self) -> Ordering {
        cmp(&self.buf, &other.buf)
    }
}

fn cmp(lhs: &[u8], rhs: &[u8]) -> Ordering {
    // Since the length of a slice is always less than or equal to
    // isize::MAX, this never underflows.
    let diff = lhs.len() as isize - rhs.len() as isize;
    // This comparison gets optimized away (on x86_64 and ARM) because the
    // subtraction updates flags.
    let mut prefix_len = if lhs.len() < rhs.len() {
        lhs.len()
    } else {
        rhs.len()
    };
    // strip null terminator
    if cfg!(unix) {
        prefix_len = prefix_len.saturating_sub(1);
    }
    // for some reason llvm fails to emti these boundschecks and since we need fast sorting
    // we use some unsafe
    let lhs_ = unsafe { slice::from_raw_parts(lhs.as_ptr(), prefix_len) };
    let rhs_ = unsafe { slice::from_raw_parts(rhs.as_ptr(), prefix_len) };
    lhs_.cmp(rhs_).then_with(|| match diff.cmp(&0) {
        Ordering::Less => PATH_SEPERATOR.cmp(unsafe { rhs.get_unchecked(prefix_len) }),
        Ordering::Equal => Ordering::Equal,
        Ordering::Greater => unsafe { lhs.get_unchecked(prefix_len) }.cmp(&PATH_SEPERATOR),
    })
}

impl CannonicalPathBuf {
    pub fn new() -> CannonicalPathBuf {
        Self { buf: EcoVec::new() }
    }

    pub fn assert_cannoncalized(path: &Path) -> CannonicalPathBuf {
        let path = path.as_os_str();
        let mut res = Self::new();
        res.push(path);
        res
    }

    // pub fn from_std_path(path: &Path) -> io::Result<CannonicalPathBuf> {
    //     let cannoncalized = path.canonicalize()?.into_os_string();
    //     let mut res = Self::with_capacity(cannoncalized.len() + 1);
    //     res.push(cannoncalized.as_os_str());
    //     Ok(res)
    // }

    fn with_capacity(cap: usize) -> CannonicalPathBuf {
        Self {
            buf: EcoVec::with_capacity(cap),
        }
    }

    pub fn pop(&mut self) -> bool {
        let Some(i) = memrchr(PATH_SEPERATOR, &self.bytes) else {
            return false;
        };
        self.buf.truncate(i);
        true
    }

    pub fn push_raw(&mut self, src: impl AsRef<OsStr>) {
        let src = src.as_ref();
        let mut capacity = src.len();
        if cfg!(unix) {
            // remove null
            let removed = self.buf.pop();
            debug_assert!(removed.is_none_or(|c| c == 0));
            capacity += 1;
        }
        self.buf.reserve(capacity + 1);
        self.buf.extend_from_slice(src.as_encoded_bytes());
        if cfg!(unix) {
            self.buf.push(0);
        }
    }

    pub fn push(&mut self, src: impl AsRef<OsStr>) {
        let src = src.as_ref();
        let mut capacity = src.len();
        // we append the null terminator only on unix
        if cfg!(unix) {
            // remove null
            let removed = self.buf.pop();
            debug_assert!(removed.is_none_or(|c| c == 0));
            capacity += 1;
        }
        self.buf.reserve(capacity + 1);
        if src.as_encoded_bytes().first() != Some(&PATH_SEPERATOR) {
            self.buf.push(PATH_SEPERATOR);
        }
        self.buf.extend_from_slice(src.as_encoded_bytes());
        if cfg!(unix) {
            self.buf.push(0);
        }
    }
}

impl Default for CannonicalPathBuf {
    fn default() -> Self {
        Self::new()
    }
}

impl Deref for CannonicalPathBuf {
    type Target = CannonicalPath;

    fn deref(&self) -> &Self::Target {
        // safety: repr(transparent)
        unsafe { transmute(self.buf.as_slice()) }
    }
}

impl Debug for CannonicalPathBuf {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.as_std_path().fmt(f)
    }
}

impl Display for CannonicalPathBuf {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Display::fmt(&self.as_std_path().display(), f)
    }
}

impl Debug for CannonicalPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.as_std_path().fmt(f)
    }
}

impl Display for CannonicalPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Display::fmt(&self.as_std_path().display(), f)
    }
}

#[cfg(unix)]
impl rustix::path::Arg for &CannonicalPath {
    fn as_str(&self) -> rustix::io::Result<&str> {
        self.as_os_str().to_str().ok_or(rustix::io::Errno::INVAL)
    }

    fn to_string_lossy(&self) -> std::borrow::Cow<'_, str> {
        self.as_std_path().to_string_lossy()
    }

    fn as_cow_c_str(&self) -> rustix::io::Result<std::borrow::Cow<'_, std::ffi::CStr>> {
        Ok(self.as_c_str().into())
    }

    fn into_c_str<'b>(self) -> rustix::io::Result<std::borrow::Cow<'b, std::ffi::CStr>>
    where
        Self: 'b,
    {
        Ok(unsafe {
            use std::ffi::CString;
            CString::from_vec_with_nul_unchecked(Vec::from(&self.bytes)).into()
        })
    }

    fn into_with_c_str<T, F>(self, f: F) -> rustix::io::Result<T>
    where
        Self: Sized,
        F: FnOnce(&std::ffi::CStr) -> rustix::io::Result<T>,
    {
        f(self.as_c_str())
    }
}

// don't include the null terminator for Hash so that we
// can lookup a normal path aswell
impl Hash for CannonicalPath {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_bytes().hash(state);
    }
}

impl Hash for CannonicalPathBuf {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_bytes().hash(state);
    }
}

impl<T: AsRef<OsStr>> PartialEq<T> for CannonicalPath {
    fn eq(&self, other: &T) -> bool {
        self.as_os_str() == other.as_ref()
    }
}

impl<T: AsRef<OsStr>> PartialEq<T> for CannonicalPathBuf {
    fn eq(&self, other: &T) -> bool {
        self.as_os_str() == other.as_ref()
    }
}
