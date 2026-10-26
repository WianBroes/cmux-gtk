//! Local network interface addresses for explicitly bounded host services.

use std::io;
use std::net::Ipv4Addr;

/// Return every local IPv4 address assigned to an interface, sorted and deduplicated.
///
/// The kernel interface list is copied once; no DNS lookup or external command runs.
/// Callers decide which addresses are safe to bind (for example, loopback or a VPN range).
pub fn ipv4_addresses() -> io::Result<Vec<Ipv4Addr>> {
    let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: `getifaddrs` initializes the list on success; on failure the pointer is unused.
    if unsafe { libc::getifaddrs(&mut head) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let mut addresses = Vec::new();
    let mut cursor = head;
    while !cursor.is_null() {
        // SAFETY: every node in a `getifaddrs` result is a valid linked list entry.
        let entry = unsafe { &*cursor };
        if !entry.ifa_addr.is_null() {
            // SAFETY: the family tag is present in every returned sockaddr.
            let family = unsafe { (*entry.ifa_addr).sa_family as i32 };
            if family == libc::AF_INET {
                // SAFETY: an AF_INET address points at a complete `sockaddr_in`.
                let address = unsafe { &*(entry.ifa_addr as *const libc::sockaddr_in) };
                // The stored word's memory bytes are already in network order.
                addresses.push(Ipv4Addr::from(address.sin_addr.s_addr.to_ne_bytes()));
            }
        }
        // SAFETY: the list is owned by `head` and remains alive until `freeifaddrs`.
        cursor = unsafe { (*cursor).ifa_next };
    }
    // SAFETY: `head` came from a successful `getifaddrs` call and is freed exactly once.
    unsafe { libc::freeifaddrs(head) };
    addresses.sort();
    addresses.dedup();
    Ok(addresses)
}
