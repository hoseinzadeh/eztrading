use std::sync::atomic::{AtomicU64, Ordering};

use clap::Parser;
use raw_sync::locks::*;
use shared_memory::*;

/// Spawns N threads that increment a value to 10 using a mutex
#[derive(Parser)]
#[clap(author, version, about)]
struct Args {
    /// Number of threads to spawn
    num_threads: usize,

    /// Count to this value
    #[clap(long, short, default_value_t = 50)]
    count_to: u8,
}

fn main() {
    env_logger::init();

    increment_value("mutex_mapping");
}

fn increment_value(shmem_flink: &str) {
    // Create or open the shared memory mapping
    let shmem = match ShmemConf::new().size(4096).flink(shmem_flink).create() {
        Ok(m) => m,
        Err(ShmemError::LinkExists) => ShmemConf::new().flink(shmem_flink).open().unwrap(),
        Err(e) => {
            eprintln!(
                "Unable to create or open shmem flink {} : {}",
                shmem_flink, e
            );
            return;
        }
    };

    let mut raw_ptr = shmem.as_ptr();
    let is_init: &mut AtomicU64;

    unsafe {
        is_init = &mut *(raw_ptr as *mut u64 as *mut AtomicU64);
        raw_ptr = raw_ptr.add(8);
    };

    // Initialize or wait for initialized mutex
    let mutex = if shmem.is_owner() {
        is_init.store(0, Ordering::Relaxed);
        // Initialize the mutex
        let (lock, _bytes_used) = unsafe {
            Mutex::new(
                raw_ptr,                                    // Base address of Mutex
                raw_ptr.add(Mutex::size_of(Some(raw_ptr))), // Address of data protected by mutex
            )
            .unwrap()
        };
        is_init.store(1, Ordering::Relaxed);
        lock
    } else {
        // wait until mutex is initialized
        while is_init.load(Ordering::Relaxed) != 1 {}
        // Load existing mutex
        let (lock, _bytes_used) = unsafe {
            Mutex::from_existing(
                raw_ptr,                                    // Base address of Mutex
                raw_ptr.add(Mutex::size_of(Some(raw_ptr))), // Address of data  protected by mutex
            )
            .unwrap()
        };
        lock
    };

    // Loop until mutex data reaches 10
    for _ in 1..10000 {
        // Scope where mutex will be locked
        {
            let guard = mutex.lock().unwrap();
            // Cast mutex data to &mut u8
            let val: &mut u64 = unsafe { &mut *(*guard as *mut u64) };

            // Print contents and increment value
            println!("Process {} Val : {}", std::process::id(), *val);
            *val += 1;

            // Hold lock for a second
            std::thread::sleep(std::time::Duration::from_micros(100));
        }

        // Timeout this thread for a second
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}