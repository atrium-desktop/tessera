//! Unix domain socket transport, D-Bus address parsing, SASL EXTERNAL auth,
//! and `SCM_RIGHTS` file-descriptor passing.

use std::collections::VecDeque;
use std::io::{self, BufRead, BufReader, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;

use crate::wire::{Message, WireError, frame_total_len};

/// Parse a D-Bus address (`unix:path=...` or `unix:abstract=...`) and connect.
pub fn connect_address(addr: &str) -> io::Result<UnixStream> {
    for entry in addr.split(';') {
        let entry = entry.trim();
        let Some(params) = entry.strip_prefix("unix:") else {
            continue;
        };
        for kv in params.split(',') {
            if let Some(path) = kv.strip_prefix("path=") {
                let decoded = unescape_dbus_addr(path);
                return UnixStream::connect(decoded);
            }
            if let Some(abs) = kv.strip_prefix("abstract=") {
                let decoded = unescape_dbus_addr(abs);
                return connect_abstract_unix(decoded.as_bytes());
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("unsupported D-Bus address: {addr:?}"),
    ))
}

fn connect_abstract_unix(name: &[u8]) -> io::Result<UnixStream> {
    let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let owned = unsafe { OwnedFd::from_raw_fd(fd) };
    let mut sa: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    sa.sun_family = libc::AF_UNIX as libc::sa_family_t;
    if 1 + name.len() > sa.sun_path.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "abstract socket name too long",
        ));
    }
    sa.sun_path[0] = 0;
    for (i, &b) in name.iter().enumerate() {
        sa.sun_path[1 + i] = b as libc::c_char;
    }
    let len =
        (std::mem::offset_of!(libc::sockaddr_un, sun_path) + 1 + name.len()) as libc::socklen_t;
    let rc = unsafe {
        libc::connect(
            owned.as_raw_fd(),
            (&raw const sa).cast::<libc::sockaddr>(),
            len,
        )
    };
    if rc < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(UnixStream::from(owned))
}

fn unescape_dbus_addr(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2]))
        {
            out.push((hi << 4) | lo);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Perform D-Bus `SASL EXTERNAL` authentication and optional UNIX FD negotiation.
pub fn authenticate(stream: &mut UnixStream) -> io::Result<bool> {
    let uid = unsafe { libc::geteuid() };
    let uid_str = uid.to_string();
    let mut hex_uid = String::with_capacity(uid_str.len() * 2);
    for b in uid_str.as_bytes() {
        use std::fmt::Write as _;
        let _ = write!(&mut hex_uid, "{b:02x}");
    }

    stream.write_all(format!("\0AUTH EXTERNAL {hex_uid}\r\n").as_bytes())?;
    stream.flush()?;

    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    if !line.starts_with("OK ") {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("D-Bus SASL EXTERNAL rejected: {}", line.trim()),
        ));
    }

    stream.write_all(b"NEGOTIATE_UNIX_FD\r\n")?;
    stream.flush()?;
    line.clear();
    reader.read_line(&mut line)?;
    let unix_fd_agreed = line.starts_with("AGREE_UNIX_FD");

    stream.write_all(b"BEGIN\r\n")?;
    stream.flush()?;
    Ok(unix_fd_agreed)
}

pub struct Transport {
    stream: UnixStream,
    recv_fds: VecDeque<OwnedFd>,
}

impl Transport {
    pub fn new(stream: UnixStream) -> Self {
        Self {
            stream,
            recv_fds: VecDeque::new(),
        }
    }

    pub fn raw_fd(&self) -> RawFd {
        self.stream.as_raw_fd()
    }

    pub fn send_message(&mut self, msg: &Message) -> Result<(), TransportError> {
        let (bytes, fds) = msg.encode()?;
        let raw_fds: Vec<RawFd> = fds.iter().map(|f| f.as_raw_fd()).collect();
        self.send_all_with_fds(&bytes, &raw_fds)?;
        Ok(())
    }

    pub fn read_message(&mut self) -> Result<Message, TransportError> {
        let mut prefix = [0u8; 16];
        self.read_exact_with_fds(&mut prefix)?;
        let total = frame_total_len(&prefix)?;
        if !(16..=128 * 1024 * 1024).contains(&total) {
            return Err(TransportError::Wire(WireError::InvalidFormat(format!(
                "invalid D-Bus frame length {total}"
            ))));
        }
        let mut buf = vec![0u8; total];
        buf[..16].copy_from_slice(&prefix);
        if total > 16 {
            self.read_exact_with_fds(&mut buf[16..])?;
        }
        let fds: Vec<OwnedFd> = self.recv_fds.drain(..).collect();
        Ok(Message::decode(&buf, fds)?)
    }

    fn send_all_with_fds(&mut self, mut data: &[u8], fds: &[RawFd]) -> io::Result<()> {
        let mut fds_sent = false;
        while !data.is_empty() {
            let chunk = if fds.is_empty() || fds_sent {
                self.stream.write(data)
            } else {
                fds_sent = true;
                send_with_fds(self.stream.as_raw_fd(), data, fds)
            };
            match chunk {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "D-Bus socket closed on write",
                    ));
                }
                Ok(n) => data = &data[n..],
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    fn read_exact_with_fds(&mut self, mut dst: &mut [u8]) -> io::Result<()> {
        while !dst.is_empty() {
            match recv_with_fds(self.stream.as_raw_fd(), dst, &mut self.recv_fds) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "D-Bus socket closed on read",
                    ));
                }
                Ok(n) => {
                    let tmp = dst;
                    dst = &mut tmp[n..];
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
}

#[derive(Debug)]
pub enum TransportError {
    Io(io::Error),
    Wire(WireError),
}

impl From<io::Error> for TransportError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<WireError> for TransportError {
    fn from(e: WireError) -> Self {
        Self::Wire(e)
    }
}

fn send_with_fds(sock: RawFd, data: &[u8], fds: &[RawFd]) -> io::Result<usize> {
    let mut iov = libc::iovec {
        iov_base: data.as_ptr().cast::<libc::c_void>().cast_mut(),
        iov_len: data.len(),
    };
    let fd_bytes = std::mem::size_of_val(fds);
    let cmsg_space = unsafe { libc::CMSG_SPACE(fd_bytes as libc::c_uint) } as usize;
    let mut cmsg_buf = vec![0u8; cmsg_space];

    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &raw mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = cmsg_buf.as_mut_ptr().cast::<libc::c_void>();
    msg.msg_controllen = cmsg_space as _;

    unsafe {
        let cmsg = libc::CMSG_FIRSTHDR(&raw const msg);
        (*cmsg).cmsg_level = libc::SOL_SOCKET;
        (*cmsg).cmsg_type = libc::SCM_RIGHTS;
        (*cmsg).cmsg_len = libc::CMSG_LEN(fd_bytes as libc::c_uint) as _;
        std::ptr::copy_nonoverlapping(fds.as_ptr().cast::<u8>(), libc::CMSG_DATA(cmsg), fd_bytes);
    }

    let n = unsafe { libc::sendmsg(sock, &raw const msg, libc::MSG_NOSIGNAL) };
    if n < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(n as usize)
    }
}

fn recv_with_fds(
    sock: RawFd,
    dst: &mut [u8],
    out_fds: &mut VecDeque<OwnedFd>,
) -> io::Result<usize> {
    let mut iov = libc::iovec {
        iov_base: dst.as_mut_ptr().cast::<libc::c_void>(),
        iov_len: dst.len(),
    };
    let mut cmsg_buf = [0u8; 512];
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &raw mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = cmsg_buf.as_mut_ptr().cast::<libc::c_void>();
    msg.msg_controllen = cmsg_buf.len() as _;

    let n = unsafe { libc::recvmsg(sock, &raw mut msg, libc::MSG_CMSG_CLOEXEC) };
    if n < 0 {
        return Err(io::Error::last_os_error());
    }

    unsafe {
        let mut cmsg = libc::CMSG_FIRSTHDR(&raw const msg);
        while !cmsg.is_null() {
            if (*cmsg).cmsg_level == libc::SOL_SOCKET && (*cmsg).cmsg_type == libc::SCM_RIGHTS {
                let data_len = (*cmsg).cmsg_len as usize - libc::CMSG_LEN(0) as usize;
                let count = data_len / std::mem::size_of::<RawFd>();
                let ptr = libc::CMSG_DATA(cmsg).cast::<RawFd>();
                for i in 0..count {
                    let fd = std::ptr::read_unaligned(ptr.add(i));
                    out_fds.push_back(OwnedFd::from_raw_fd(fd));
                }
            }
            cmsg = libc::CMSG_NXTHDR(&raw const msg, cmsg);
        }
    }

    Ok(n as usize)
}
