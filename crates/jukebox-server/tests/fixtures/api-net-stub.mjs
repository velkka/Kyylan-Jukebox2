// The network facts the API parity harness fixes, so recorded responses don't depend on the
// machine: tests/api_electron.rs applies the same rules on the Rust side.

/** The host's LAN addresses. */
export function lanAddresses() {
  return ['192.0.2.10']
}

/**
 * A client's hostname: the host's own name for loopback, a device name for addresses below
 * .50 in the test range, and — like a failed lookup — the address itself for the rest.
 */
export async function hostnameFor(ip) {
  if (ip === '127.0.0.1') return 'jukebox-host'
  const match = /^192\.0\.2\.(\d+)$/.exec(ip)
  if (match && Number(match[1]) < 50) return 'device-' + match[1]
  return ip
}
