#![no_std]
#![cfg_attr(not(test), no_main)]
#![no_builtins]
#![feature(concat_bytes, const_trait_impl)]

use core::{
    alloc::GlobalAlloc, arch::naked_asm, cell::Cell, mem::MaybeUninit, panic::PanicInfo,
    ptr::null_mut,
};

use draw::draw_time;
use io::{ArrayWriter, BufWriter, FdWriter, Write as _};
use io_uring::IoUring;

pub mod draw;
pub mod io;
pub mod io_uring;
// pub mod zoneinfo;

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {
        core::fmt::Write::write_fmt(&mut crate::io::FdWriter::stdout(), format_args!($($arg)*)).unwrap()
    }
}

#[macro_export]
macro_rules! eprint {
    ($($arg:tt)*) => {
        core::fmt::Write::write_fmt(&mut crate::io::FdWriter::stderr(), format_args!($($arg)*)).unwrap()
    }
}

#[macro_export]
macro_rules! set_buffer {
    () => {
        b"[?1049h"
    };
}

#[macro_export]
macro_rules! restore_buffer {
    () => {
        b"[?1049l"
    };
}

#[macro_export]
macro_rules! hide_cursor {
    () => {
        b"[?25l"
    };
}

#[macro_export]
macro_rules! show_cursor {
    () => {
        b"[?25h"
    };
}

#[macro_export]
macro_rules! cursor_position {
    () => {
        b"[H"
    };
}

#[macro_export]
macro_rules! buffer_size {
    () => {
        b"[18t"
    };
}

#[macro_export]
macro_rules! fg_color {
    (black) => {
        b"[30m"
    };
    (red) => {
        b"[31m"
    };
    (green) => {
        b"[32m"
    };
    (yellow) => {
        b"[33m"
    };
    (blue) => {
        b"[34m"
    };
    (magenta) => {
        b"[35m"
    };
    (cyan) => {
        b"[36m"
    };
    (white) => {
        b"[37m"
    };

    (br_black) => {
        b"[90m"
    };
    (br_red) => {
        b"[91m"
    };
    (br_green) => {
        b"[92m"
    };
    (br_yellow) => {
        b"[93m"
    };
    (br_blue) => {
        b"[94m"
    };
    (br_magenta) => {
        b"[95m"
    };
    (br_cyan) => {
        b"[96m"
    };
    (br_white) => {
        b"[97m"
    };
}

#[inline(always)]
fn on_exit() -> io::Result<()> {
    FdWriter::stdout().write_all(concat_bytes!(restore_buffer!(), show_cursor!()))?;

    #[allow(static_mut_refs)]
    unsafe {
        nc::ioctl(io::STDIN, nc::TCSETS, TERMIOS.as_ptr() as _)?;
    }

    Ok(())
}

#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
extern "C" fn restorer() {
    naked_asm!("mov rax, 0xf", "syscall")
}

struct MarginBuf {
    buf: [u8; 32],
    len: u8,
}

impl MarginBuf {
    fn slice(&self) -> &[u8] {
        unsafe { self.buf.get_unchecked(..self.len as _) }
    }

    fn cursor_move(&mut self, n: usize, direction: Direction) -> io::Result<()> {
        let mut writer = ArrayWriter::new(&mut self.buf);
        cursor_move(&mut writer, n as _, direction)?;
        self.len = writer.len as _;
        Ok(())
    }
}

fn resize() -> io::Result<()> {
    let winsz = MaybeUninit::<nc::winsize_t>::uninit();
    #[allow(static_mut_refs)]
    unsafe {
        nc::ioctl(io::STDIN, nc::TIOCGWINSZ, winsz.as_ptr() as _).unwrap_or_else(|e| exit(e as _));
        let nc::winsize_t { ws_row, ws_col, .. } = winsz.assume_init_ref();

        MARGIN_LEFT
            .assume_init_mut()
            .cursor_move(((ws_col - 38) / 2) as _, Direction::Right)?;
        MARGIN_TOP
            .assume_init_mut()
            .cursor_move(((ws_row - 5) / 2) as _, Direction::Down)?;
    };
    Ok(())
}

fn set_signal_handler() {
    extern "C" fn terminate(_: i32) {
        _ = on_exit();
        exit(0);
    }

    unsafe {
        let sa = nc::sigaction_t {
            sa_handler: terminate as *const () as _,
            sa_flags: nc::SA_RESTORER,
            sa_restorer: None,
            ..Default::default()
        };
        _ = nc::rt_sigaction(nc::SIGINT, Some(&sa), None);
        _ = nc::rt_sigaction(nc::SIGTERM, Some(&sa), None);

        let sa = nc::sigaction_t {
            sa_handler: resize as *const () as _,
            sa_flags: nc::SA_RESTORER | nc::SA_RESTART,
            sa_restorer: Some(restorer),
            sa_mask: nc::sigset_t {
                sig: [1 << (nc::SIGWINCH) - 1],
            },
            ..Default::default()
        };
        _ = nc::rt_sigaction(nc::SIGWINCH, Some(&sa), None);
    }
}

static mut TERMIOS: MaybeUninit<nc::termios_t> = MaybeUninit::uninit();
static mut MARGIN_LEFT: MaybeUninit<MarginBuf> = MaybeUninit::uninit();
static mut MARGIN_TOP: MaybeUninit<MarginBuf> = MaybeUninit::uninit();

fn margin_left() -> &'static [u8] {
    #[allow(static_mut_refs)]
    unsafe { MARGIN_LEFT.assume_init_ref() }.slice()
}

fn margin_top() -> &'static [u8] {
    #[allow(static_mut_refs)]
    unsafe { MARGIN_TOP.assume_init_ref() }.slice()
}

#[repr(u8)]
#[allow(unused)]
enum Direction {
    Up = b'A',
    Down = b'B',
    Right = b'C',
    Left = b'D',
}

fn cursor_move(writer: &mut impl io::Write, n: u64, direction: Direction) -> io::Result<()> {
    writer.write_all(b"[")?;
    writer.write_u64(n)?;
    writer.write_all(&[direction as _][..])?;
    Ok(())
}

fn main() -> io::Result<()> {
    let mut buf = MaybeUninit::<[u8; 1024]>::uninit();
    let buf = unsafe { buf.assume_init_mut() };
    let mut ctx = draw::Context::new(BufWriter::new(FdWriter::stdout(), buf));

    let get_time = || -> io::Result<isize> {
        let mut time = MaybeUninit::uninit();
        unsafe {
            nc::time(time.assume_init_mut())?;
            Ok(time.assume_init())
        }
    };

    let seconds = Cell::new(get_time()?);

    let mut redraw = || -> io::Result<()> {
        ctx.writer.write_all(concat_bytes!(
            restore_buffer!(),
            set_buffer!(),
            cursor_position!(),
            fg_color!(br_blue),
        ))?;
        ctx.writer.write_all(margin_top())?;
        let content = draw_time(seconds.get() + 8 * 3600);
        ctx.draw(Some(margin_left()), || content)?;
        ctx.writer.flush()?;
        Ok(())
    };

    #[allow(static_mut_refs)]
    unsafe {
        nc::ioctl(io::STDIN, nc::TCGETS, TERMIOS.as_ptr() as _)?;
        let mut termios = TERMIOS.assume_init_ref().clone();
        termios.c_lflag &= !(nc::ECHO | nc::ICANON);
        nc::ioctl(io::STDIN, nc::TCSETS, &raw const termios as _)?;
    }

    resize()?;
    redraw()?;
    set_signal_handler();
    FdWriter::stdout().write_all(hide_cursor!())?;

    #[repr(usize)]
    enum Token {
        Timeout = 1,
        Read,
    }
    let ring = IoUring::new(2)?;

    let mut input_buf = MaybeUninit::<[u8; 32]>::uninit();
    ring.prepare_read(
        io::STDIN as _,
        unsafe { input_buf.assume_init_mut() },
        Token::Read as _,
    );
    let duration = nc::timespec_t {
        tv_sec: 1,
        tv_nsec: 0,
    };
    ring.prepare_timeout(&duration, Token::Timeout as _, 1 << 6); // multishot

    ring.submit(2)?;

    fn wait(ring: &IoUring, cb: &mut impl FnMut() -> io::Result<()>) -> io::Result<()> {
        loop {
            match ring.wait() {
                Ok(_) => break Ok(()),
                Err(x) if x == nc::EINTR => cb()?,
                Err(x) => break Err(x),
            }
        }
    }

    loop {
        wait(&ring, &mut redraw)?;
        let cqe = ring.complete();
        match cqe.user_data {
            x if x == Token::Timeout as _ => {
                seconds.set(get_time()?);
                redraw()?;
            }
            x if x == Token::Read as _ => {
                if cqe.res == 1 && [b'', b'q'].contains(&unsafe { input_buf.assume_init_ref() }[0])
                {
                    break;
                }
                ring.prepare_read(
                    io::STDIN as _,
                    unsafe { input_buf.assume_init_mut() },
                    Token::Read as _,
                );
            }
            _ => return Err(nc::EIO),
        }
        ring.submit(1)?;
    }
    on_exit()
}

#[cfg_attr(not(test), unsafe(no_mangle))]
extern "C" fn _start() -> ! {
    exit(match main() {
        Ok(_) => 0,
        Err(e) => e as _,
    });
}

pub fn exit(status: usize) -> ! {
    unsafe { nc::exit_group(status as _) };
}

#[cfg_attr(not(test), panic_handler)]
pub fn panic(info: &PanicInfo) -> ! {
    _ = on_exit();
    if let Some(x) = info.location() {
        eprint!("{}: ", x);
    }
    eprint!("{}\n", info.message());
    exit(1)
}

#[cfg_attr(not(test), global_allocator)]
pub static GLOBAL_ALLOCATOR: GlobalAllocator = GlobalAllocator;

pub struct GlobalAllocator;

unsafe impl GlobalAlloc for GlobalAllocator {
    unsafe fn alloc(&self, _layout: core::alloc::Layout) -> *mut u8 {
        null_mut()
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: core::alloc::Layout) {}
}

// restrict pointer
#[cfg_attr(not(test), unsafe(no_mangle))]
pub fn memcpy(dst: &mut u8, src: &u8, mut n: usize) -> *mut u8 {
    let mut dst = dst as *mut u8;
    let mut src = src as *const u8;
    while n != 0 {
        unsafe {
            *dst = *src;
            dst = dst.add(1);
            src = src.add(1);
        }
        n -= 1;
    }
    dst
}

#[cfg_attr(not(test), unsafe(no_mangle))]
pub fn memset(mut dst: *mut u8, chr: u8, mut n: usize) -> *mut u8 {
    while n != 0 {
        unsafe {
            *dst = chr;
            dst = dst.add(1);
        }
        n -= 1;
    }
    dst
}
