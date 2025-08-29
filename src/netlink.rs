use libc;
use std::io::Error;
use std::mem::zeroed;

#[macro_export]
macro_rules! errno_msg {
    ($msg:literal) => {{
        let cstr = libc::strerror(*libc::__errno_location());

        format!(
            "{}: {}",
            $msg,
            std::ffi::CStr::from_ptr(cstr).to_str().unwrap()
        )
    }};
}

pub mod utils {
    pub fn get_element_val(uevent_str: &str, name: &str) -> Option<String> {
        let delim = if uevent_str.contains("\0") {
            "\0"
        } else {
            "\n"
        };
        let target = if name == "@" {
            "@".into()
        } else {
            format!("{name}=")
        };
        let start = uevent_str
            .find(&target)
            .map(|idx| &uevent_str[idx + target.len()..])?;

        start.find(delim).map(|idx| start[..idx].to_string())
    }
}

pub enum NetlinkError<E> {
    Timeout,
    IO(std::io::ErrorKind),
    Serialize(E),
}

pub trait Uevent<E> {
    fn from_bytes(data: &Vec<u8>) -> Result<Self, E>
    where
        Self: Sized;
}

pub struct NetlinkHandle {
    fd: i32,
    buf: Vec<u8>,
}

impl NetlinkHandle {
    pub fn new() -> Result<Self, String> {
        unsafe {
            let fd = libc::socket(
                libc::AF_NETLINK,
                libc::SOCK_RAW,
                libc::NETLINK_KOBJECT_UEVENT,
            );

            if fd == -1 {
                return Err(errno_msg!("libc::socket error"));
            }

            let mut addr = zeroed::<libc::sockaddr_nl>();

            addr.nl_family = libc::AF_NETLINK as u16;
            addr.nl_groups = 1;

            if libc::bind(
                fd,
                &addr as *const _ as *const libc::sockaddr,
                size_of::<libc::sockaddr_nl>() as u32,
            ) == -1
            {
                return Err(errno_msg!("libc::bind error"));
            }

            Ok(Self {
                fd,
                buf: Vec::with_capacity(256),
            })
        }
    }

    pub fn read_uevent_msec<U: Uevent<E>, E>(
        &mut self,
        timeout: i32,
    ) -> Result<U, NetlinkError<E>> {
        unsafe {
            let mut header = zeroed::<libc::msghdr>();
            let mut iov = zeroed::<libc::iovec>();
            let mut addr = zeroed::<libc::sockaddr_nl>();
            let mut flags = libc::MSG_PEEK | libc::MSG_TRUNC;

            iov.iov_base = self.buf.as_mut_ptr() as *mut libc::c_void;
            iov.iov_len = self.buf.capacity();

            header.msg_name = &mut addr as *mut _ as *mut libc::c_void;
            header.msg_namelen = size_of::<libc::sockaddr_nl>() as u32;
            header.msg_iov = &mut iov;
            header.msg_iovlen = 1;

            // not rly necessary, just small optimization
            if timeout > 0 {
                let mut pfd = zeroed::<libc::pollfd>();

                pfd.fd = self.fd;
                pfd.events = libc::POLLIN;

                match libc::poll(&mut pfd, 1, timeout) {
                    i if i == 0 => {
                        return Err(NetlinkError::Timeout);
                    }
                    i if i == -1 => {
                        return Err(NetlinkError::IO(Error::last_os_error().kind()));
                    }
                    _ => (), // ready to read
                }
            }

            loop {
                match libc::recvmsg(self.fd, &mut header, flags) {
                    i if i == -1 => return Err(NetlinkError::IO(Error::last_os_error().kind())),
                    i => {
                        if i > self.buf.capacity() as isize {
                            self.buf.resize(i as usize * 2, 0);

                            iov.iov_base = self.buf.as_mut_ptr() as *mut libc::c_void;
                            iov.iov_len = self.buf.capacity();
                        }

                        self.buf.set_len(i as usize);

                        if flags & libc::MSG_DONTWAIT == 0 {
                            flags ^= libc::MSG_PEEK | libc::MSG_TRUNC | libc::MSG_DONTWAIT;
                        } else {
                            return U::from_bytes(&self.buf)
                                .map_err(|e| NetlinkError::Serialize(e));
                        }
                    }
                }
            }
        }
    }

    pub fn read_uevent<U: Uevent<E>, E>(&mut self) -> Result<U, NetlinkError<E>> {
        self.read_uevent_msec(-1)
    }
}
