// OpenBazaar: public Lightning peers are dialed through the node's SOCKS proxy
// (`socks5h`, so the proxy resolves the name). A loopback or LAN host stays a
// direct TCP connection. There is no fallback to a clearnet dial when the
// proxy is set and the handshake fails.

use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream, ToSocketAddrs};
use std::time::Duration;

use lightning::ln::msgs::SocketAddress;

use crate::Error;

/// A host on this machine or the LAN. A public name is not.
pub(crate) fn host_is_local(text: &str) -> bool {
	let text = text.split("://").last().unwrap_or(text);
	let text = text.split(['/', '?', '#']).next().unwrap_or(text);
	let host = if let Some(rest) = text.strip_prefix('[') {
		rest.split(']').next().unwrap_or("")
	} else {
		text.rsplit_once(':')
			.map(|(host, port)| if port.parse::<u16>().is_ok() { host } else { text })
			.unwrap_or(text)
	};
	if host.eq_ignore_ascii_case("localhost") || host.ends_with(".local") {
		return true;
	}
	match host.parse::<IpAddr>() {
		Ok(IpAddr::V4(ip)) => ip.is_loopback() || ip.is_private() || ip.is_link_local(),
		Ok(IpAddr::V6(ip)) => ip.is_loopback() || (ip.segments()[0] & 0xfe00) == 0xfc00,
		Err(_) => false,
	}
}

/// The host and port LDK should dial, without resolving DNS here.
pub(crate) fn peer_host(addr: &SocketAddress) -> Result<(String, u16), Error> {
	match addr {
		SocketAddress::TcpIpV4 { addr, port } => Ok((Ipv4Addr::from(*addr).to_string(), *port)),
		SocketAddress::TcpIpV6 { addr, port } => Ok((Ipv6Addr::from(*addr).to_string(), *port)),
		SocketAddress::Hostname { hostname, port } => Ok((hostname.to_string(), *port)),
		SocketAddress::OnionV3 { .. } => {
			let text = addr.to_string();
			let (host, port) = text.rsplit_once(':').ok_or(Error::InvalidSocketAddress)?;
			let port = port.parse().map_err(|_| Error::InvalidSocketAddress)?;
			Ok((host.to_string(), port))
		},
		SocketAddress::OnionV2(_) => Err(Error::InvalidSocketAddress),
	}
}

/// Open a TCP connection to `host:port` through `proxy` (`socks5h://HOST:PORT`).
/// The proxy sees the peer; this machine's DNS does not.
pub fn connect_via_socks(proxy: &str, host: &str, port: u16) -> std::io::Result<TcpStream> {
	if host.is_empty() || host.len() > 255 {
		return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "peer host is not a SOCKS name"));
	}
	let mut stream = TcpStream::connect_timeout(&proxy_addr(proxy)?, Duration::from_secs(10))?;
	stream.set_read_timeout(Some(Duration::from_secs(10)))?;
	stream.set_write_timeout(Some(Duration::from_secs(10)))?;
	stream.write_all(&[0x05, 0x01, 0x00])?;
	let mut greet = [0u8; 2];
	stream.read_exact(&mut greet)?;
	if greet != [0x05, 0x00] {
		return Err(std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "SOCKS proxy refused"));
	}
	let mut req = Vec::with_capacity(host.len() + 22);
	req.extend_from_slice(&[0x05, 0x01, 0x00]);
	match host.parse::<IpAddr>() {
		Ok(IpAddr::V4(ip)) => {
			req.push(0x01);
			req.extend_from_slice(&ip.octets());
		},
		Ok(IpAddr::V6(ip)) => {
			req.push(0x04);
			req.extend_from_slice(&ip.octets());
		},
		Err(_) => {
			req.push(0x03);
			req.push(host.len() as u8);
			req.extend_from_slice(host.as_bytes());
		},
	}
	req.extend_from_slice(&port.to_be_bytes());
	stream.write_all(&req)?;
	let mut hdr = [0u8; 4];
	stream.read_exact(&mut hdr)?;
	if hdr[0] != 0x05 || hdr[1] != 0x00 {
		return Err(std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "SOCKS connect failed"));
	}
	let extra = match hdr[3] {
		0x01 => 4,
		0x04 => 16,
		0x03 => {
			let mut len = [0u8; 1];
			stream.read_exact(&mut len)?;
			len[0] as usize
		},
		_ => {
			return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "SOCKS reply address"));
		},
	};
	let mut tail = vec![0u8; extra + 2];
	stream.read_exact(&mut tail)?;
	Ok(stream)
}

fn proxy_addr(proxy: &str) -> std::io::Result<SocketAddr> {
	let rest = proxy.strip_prefix("socks5h://").or_else(|| proxy.strip_prefix("socks5://")).unwrap_or(proxy);
	let rest = rest.trim_end_matches('/');
	if rest.contains('@') {
		return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "SOCKS userinfo is not used"));
	}
	rest.to_socket_addrs()?.next().ok_or_else(|| {
		std::io::Error::new(std::io::ErrorKind::InvalidInput, "proxy has no address")
	})
}
