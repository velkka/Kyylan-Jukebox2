//! What the server asks of the network and the host: its LAN addresses, guests' hostnames,
//! and the native folder picker. Behind traits so the parity harness can fix the answers.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;

pub trait Network: Send + Sync {
    /// This host's non-internal IPv4 addresses, for guest links.
    fn lan_addresses(&self) -> Vec<String>;
    /// A display name for a guest. May block on a lookup.
    fn hostname(&self, ip: &str) -> String;
}

/// The real network. Mirrors net.ts.
#[derive(Default)]
pub struct SystemNetwork {
    /// Lookups are slow and often fail on a LAN, so each answer — including falling back
    /// to the address — is kept for the life of the process.
    hostnames: Mutex<HashMap<String, String>>,
}

impl Network for SystemNetwork {
    fn lan_addresses(&self) -> Vec<String> {
        if_addrs::get_if_addrs()
            .unwrap_or_default()
            .into_iter()
            .filter(|i| !i.is_loopback())
            .filter_map(|i| match i.ip() {
                IpAddr::V4(v4) => Some(v4.to_string()),
                IpAddr::V6(_) => None,
            })
            .collect()
    }

    fn hostname(&self, ip: &str) -> String {
        if let Some(name) = self
            .hostnames
            .lock()
            .expect("hostname lock poisoned")
            .get(ip)
        {
            return name.clone();
        }
        let name = if ip == "127.0.0.1" {
            dns_lookup::get_hostname()
                .map(|h| short_hostname(&h))
                .unwrap_or_else(|_| ip.into())
        } else {
            // The OS resolver, as Node's lookupService used, so mDNS `.local` names resolve.
            ip.parse::<IpAddr>()
                .ok()
                .and_then(|addr| dns_lookup::lookup_addr(&addr).ok())
                .filter(|host| host != ip)
                .map(|host| short_hostname(&host))
                .unwrap_or_else(|| ip.into())
        };
        self.hostnames
            .lock()
            .expect("hostname lock poisoned")
            .insert(ip.to_string(), name.clone());
        name
    }
}

/// Just the device part: `Velkkas-iPhone.local.` → `Velkkas-iPhone`.
pub fn short_hostname(name: &str) -> String {
    let stripped = name.strip_suffix('.').unwrap_or(name);
    match stripped.split('.').next() {
        Some(first) if !first.is_empty() => first.to_string(),
        _ => name.to_string(),
    }
}

/// The host's "choose a music folder" dialog.
pub trait FolderPicker: Send + Sync {
    /// The chosen folder, or `None` when cancelled. Blocks until the dialog closes.
    fn pick_folder(&self) -> Option<String>;
}

/// No dialog to show: every pick is cancelled. Used until the tray build provides one, and
/// where there is no screen.
pub struct NoFolderPicker;

impl FolderPicker for NoFolderPicker {
    fn pick_folder(&self) -> Option<String> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostnames_shorten_like_net_ts() {
        assert_eq!(short_hostname("Velkkas-iPhone.local."), "Velkkas-iPhone");
        assert_eq!(short_hostname("laptop"), "laptop");
        assert_eq!(short_hostname(".local."), ".local.");
    }
}
