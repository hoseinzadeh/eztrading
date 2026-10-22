use std::ffi::c_void;

use com::{status::Status, failed};

pub struct Scheduler {
    handle: *mut c_void,
    running_machines: Vec<*mut c_void>
}

impl Scheduler {
    pub fn new(handle: *mut c_void) -> Self {
        Self{ handle, running_machines: vec![] }
    }
    
    pub fn handle(&self) -> *mut c_void {
        self.handle
    }

    pub fn add_machine(&mut self, machine_handle: *mut c_void) -> Result<(), Status> {
        if machine_handle.is_null() {
            Err(failed!("Could not run the machine"))
        } else {
            self.running_machines.push(machine_handle);
            Ok(())
        }
    }
}
