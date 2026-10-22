use std::{mem::size_of, ptr::{null, null_mut}};



const BUF_SIZE: usize = 1024;
const SHM_KEY: i32 = 0x1234;

#[repr(C)]
struct shmseg {
    cnt: i32,
    complete: i32,
    buf: [libc::c_char; BUF_SIZE]
}

unsafe fn fill_buffer(bufptr: &mut [libc::c_char], size: usize) -> i32 {
    static mut CH: libc::c_char = 'A' as libc::c_char;
    
    //printf("size is %d\n", size);
    libc::memset(bufptr.as_mut_ptr() as *mut libc::c_void, CH as i32, size - 1);
    bufptr[size-1] = '\0' as libc::c_char;
    if CH > 122 {
        CH = 65;
    }
    if (CH >= 65) && (CH <= 122) {
       if (CH >= 91) && (CH <= 96) {
          CH = 65;
       }
    }
    let filled_count = libc::strlen(bufptr.as_ptr() as *const libc::c_char) as i32;
    
    //printf("buffer count is: %d\n", filled_count);
    //printf("buffer filled is:%s\n", bufptr);
    CH += 1;
    filled_count
 }
 
fn main() {
    unsafe {
        let shmid = libc::shmget(SHM_KEY, size_of::<shmseg>(), 0o644|libc::IPC_CREAT);
        assert!(shmid != -1, "Shared memory");

        // Attach to the segment to get a pointer to it.
        let shmp = libc::shmat(shmid, null(), 0) as *mut shmseg;
        assert!(shmp as i32 != -1, "Shared memory attach");
        let shmp = &mut *shmp;
        let mut spaceavailable = BUF_SIZE;

        let mut n = 0;
        for numtimes in 0..5 {
            shmp.cnt = fill_buffer(shmp.buf.as_mut(), spaceavailable);
            shmp.complete = 0;
            println!("Writing Process: Shared Memory Write: Wrote {} bytes", shmp.cnt);
            spaceavailable = BUF_SIZE;
            n = numtimes;
            std::thread::sleep(std::time::Duration::from_secs(3));
        }

        println!("Writing Process: Wrote {} times", n);
        shmp.complete = 1;

        assert!(libc::shmdt(shmp as *mut _ as *mut libc::c_void) != -1, "shmdt");        
     
        assert!(libc::shmctl(shmid, libc::IPC_RMID, null_mut()) != -1, "shmctl");
        println!("Writing Process: Complete");
    }
}
