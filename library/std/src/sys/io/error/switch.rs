pub fn errno() -> i32 {
    unsafe {
        *libc::errno_loc() as i32
    }
}

pub fn is_interrupted(_code: i32) -> bool {
    false
}

pub fn decode_error_kind(_code: i32) -> crate::io::ErrorKind {
    crate::io::ErrorKind::Uncategorized
}

pub fn error_string(errno: i32) -> String {
    if errno == 0 {
        "operation successful".to_string()
    } else {
        "unknown error".to_string()
    }
}
