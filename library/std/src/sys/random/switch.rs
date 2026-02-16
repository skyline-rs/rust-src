use nnsdk::nn;

pub fn fill_bytes(bytes: &mut [u8]) {
    for chunk in bytes.chunks_mut(u32::max_value() as usize) {
        unsafe { nn::os::GenerateRandomBytes(chunk.as_mut_ptr() as _, chunk.len() as _); }
    }
}