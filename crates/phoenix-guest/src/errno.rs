//! Linux's errno values and messages, which the library expects from bionic.

pub(crate) const EPERM: i32 = 1;
pub(crate) const ENOENT: i32 = 2;
pub(crate) const EINTR: i32 = 4;
pub(crate) const EIO: i32 = 5;
pub(crate) const EBADF: i32 = 9;
pub(crate) const EAGAIN: i32 = 11;
pub(crate) const ENOMEM: i32 = 12;
pub(crate) const EACCES: i32 = 13;
pub(crate) const EEXIST: i32 = 17;
pub(crate) const ENOTDIR: i32 = 20;
pub(crate) const EISDIR: i32 = 21;
pub(crate) const EINVAL: i32 = 22;
pub(crate) const EMFILE: i32 = 24;
pub(crate) const ENOTTY: i32 = 25;
pub(crate) const ERANGE: i32 = 34;
pub(crate) const EDEADLK: i32 = 35;
pub(crate) const ENAMETOOLONG: i32 = 36;
pub(crate) const ENOSYS: i32 = 38;
pub(crate) const ENOTEMPTY: i32 = 39;
pub(crate) const EILSEQ: i32 = 84;
pub(crate) const ETIMEDOUT: i32 = 110;

/// Linux's message for the errnos the shims produce.
pub(crate) fn text(e: i32) -> Option<&'static str> {
    Some(match e {
        0 => "Success",
        EPERM => "Operation not permitted",
        ENOENT => "No such file or directory",
        EINTR => "Interrupted system call",
        EIO => "Input/output error",
        EBADF => "Bad file descriptor",
        EAGAIN => "Resource temporarily unavailable",
        ENOMEM => "Cannot allocate memory",
        EACCES => "Permission denied",
        EEXIST => "File exists",
        ENOTDIR => "Not a directory",
        EISDIR => "Is a directory",
        EINVAL => "Invalid argument",
        EMFILE => "Too many open files",
        ENOTTY => "Inappropriate ioctl for device",
        ERANGE => "Numerical result out of range",
        EDEADLK => "Resource deadlock avoided",
        ENAMETOOLONG => "File name too long",
        ENOSYS => "Function not implemented",
        ENOTEMPTY => "Directory not empty",
        EILSEQ => "Invalid or incomplete multibyte or wide character",
        ETIMEDOUT => "Connection timed out",
        _ => return None,
    })
}

/// `strerror`'s text, with glibc's wording for an unknown value.
pub(crate) fn message(e: i32) -> String {
    text(e).map_or_else(|| format!("Unknown error {e}"), str::to_string)
}
