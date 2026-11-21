//! tinyuz compression (vendored C++ library, MIT) for wireless RGB uploads.

extern "C" {
    fn tuz_compress_mem(input: *const u8, input_len: usize, output: *mut u8, output_capacity: usize, dict_size: usize) -> usize;
    fn tuz_max_compressed_size(input_len: usize) -> usize;
}

pub fn compress(input: &[u8]) -> anyhow::Result<Vec<u8>> {
    anyhow::ensure!(!input.is_empty(), "nothing to compress");
    let cap = unsafe { tuz_max_compressed_size(input.len()) };
    let mut out = vec![0u8; cap];
    let n = unsafe { tuz_compress_mem(input.as_ptr(), input.len(), out.as_mut_ptr(), out.len(), 4096) };
    anyhow::ensure!(n > 0, "tinyuz compression failed");
    out.truncate(n);
    Ok(out)
}
