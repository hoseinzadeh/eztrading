use std::{fs::OpenOptions, path::PathBuf};

fn main() {
    use std::fs::File;
    use std::io::Read;

    use memmap2::Mmap;

    let filename = "memory.bin";
    let path = PathBuf::from(filename);
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(&path)
        .unwrap();
    let mut len = file.metadata().unwrap().len() as usize;
    if len < 8 {
        len = 10 * 1024 * 1024;
        file.set_len(len as u64).unwrap();
    }

    let mut mmap = unsafe { memmap2::MmapOptions::new().map_mut(&file).unwrap() };

    for i in 0..255 {
        mmap[i] = i as u8;
    }
    unsafe {
        libc::msync(
            mmap.as_mut_ptr() as *mut libc::c_void,
            255,
            libc::MS_SYNC | libc::MS_INVALIDATE,
        );
    }
}
