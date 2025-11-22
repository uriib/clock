use core::{
    cmp::max,
    ffi::{c_uint, c_void},
    ptr,
    sync::atomic::{Ordering, fence},
};

use crate::io;

type OpCode = nc::IOURING_OP;

pub struct IoUring {
    params: nc::io_uring_params_t,
    #[allow(unused)]
    fd: u32,
    queue: *mut c_void,
    sqes: *mut nc::io_uring_sqe_t,
}

impl IoUring {
    #[inline]
    pub fn new(size: u32) -> io::Result<Self> {
        let mut params = nc::io_uring_params_t::default();
        let fd = unsafe { nc::io_uring_setup(size, &mut params)? };

        let queue_size = max(
            params.sq_off.array as usize + params.sq_entries as usize * size_of::<c_uint>(),
            params.cq_off.cqes as usize
                + params.cq_entries as usize * size_of::<nc::io_uring_cqe_t>(),
        );
        let queue = unsafe {
            nc::mmap(
                ptr::null(),
                queue_size,
                nc::PROT_READ | nc::PROT_WRITE,
                nc::MAP_SHARED | nc::MAP_POPULATE,
                fd as _,
                nc::IORING_OFF_SQ_RING,
            )
        }? as _;
        let sqes = unsafe {
            nc::mmap(
                ptr::null(),
                params.sq_entries as usize * size_of::<nc::io_uring_sqe_t>(),
                nc::PROT_READ | nc::PROT_WRITE,
                nc::MAP_SHARED | nc::MAP_POPULATE,
                fd as _,
                nc::IORING_OFF_SQES,
            )
        }? as *mut nc::io_uring_sqe_t;
        Ok(Self {
            params,
            fd,
            queue,
            sqes,
        })
    }

    pub fn prepare(
        &self,
        op_code: OpCode,
        fd: usize,
        addr: usize,
        len: usize,
        user_data: usize,
        timeout_flags: u32,
    ) {
        let tail = unsafe { self.queue.add(self.params.sq_off.tail as usize) } as *mut u32;
        let mask = unsafe { self.queue.add(self.params.sq_off.ring_mask as usize) } as *mut u32;
        let array = unsafe { self.queue.add(self.params.sq_off.array as usize) } as *mut u32;

        let index = unsafe { *tail & *mask };
        let sqe = unsafe { &mut *self.sqes.add(index as usize) };
        sqe.opcode = op_code as _;
        sqe.fd = fd as i32;
        sqe.buf_addr.addr = addr as _;
        sqe.len = len as u32;
        sqe.user_data = user_data as u64;
        sqe.other_flags.timeout_flags = timeout_flags;

        unsafe { *array.add(index as usize) = index };
        fence(Ordering::SeqCst);
        unsafe { *tail += 1 };
    }

    pub fn complete(&self) -> &nc::io_uring_cqe_t {
        let head = unsafe { self.queue.add(self.params.cq_off.head as usize) } as *mut u32;
        let mask = unsafe { self.queue.add(self.params.cq_off.ring_mask as usize) } as *mut u32;
        let cqes =
            unsafe { self.queue.add(self.params.cq_off.cqes as usize) } as *mut nc::io_uring_cqe_t;

        let cqe = unsafe { &*cqes.add((*head & *mask) as usize) };
        fence(Ordering::SeqCst);
        unsafe { *head += 1 };
        cqe
    }

    pub fn prepare_read(&self, fd: usize, buf: &mut [u8], user_data: usize) {
        self.prepare(
            OpCode::IORING_OP_READ,
            fd,
            buf.as_ptr() as usize,
            buf.len(),
            user_data,
            0,
        )
    }

    pub fn prepare_timeout(&self, duration: &nc::timespec_t, user_data: usize, flags: u32) {
        self.prepare(
            OpCode::IORING_OP_TIMEOUT,
            usize::MAX,
            duration as *const _ as usize,
            1,
            user_data,
            flags,
        );
    }

    pub fn enter(
        &self,
        to_submit: u32,
        min_complete: u32,
        flags: u32,
        sigset: *const c_void,
    ) -> io::Result<i32> {
        unsafe { nc::io_uring_enter(self.fd as _, to_submit, min_complete, flags, sigset, 8) }
    }

    fn submit_wait_mask_impl(&self, to_submit: u32, sigset: *const c_void) -> io::Result<i32> {
        self.enter(to_submit, 1, nc::IORING_ENTER_GETEVENTS, sigset)
    }

    pub fn submit_wait_mask(&self, to_submit: u32, sigset: &nc::sigset_t) -> io::Result<i32> {
        self.submit_wait_mask_impl(to_submit, sigset as *const _ as _)
    }

    pub fn submit(&self, to_submit: u32) -> io::Result<i32> {
        self.enter(to_submit, 0, 0, ptr::null())
    }

    pub fn submit_wait(&self, to_submit: u32) -> io::Result<i32> {
        self.submit_wait_mask_impl(to_submit, ptr::null())
    }

    pub fn wait(&self) -> io::Result<i32> {
        self.submit_wait_mask_impl(0, ptr::null())
    }
}
