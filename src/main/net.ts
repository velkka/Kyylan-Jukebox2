import { networkInterfaces } from 'node:os'

/** Returns non-internal IPv4 addresses of this host (for guest-facing URLs). */
export function lanAddresses(): string[] {
  const out: string[] = []
  const ifaces = networkInterfaces()
  for (const name of Object.keys(ifaces)) {
    for (const info of ifaces[name] ?? []) {
      if (info.family === 'IPv4' && !info.internal) {
        out.push(info.address)
      }
    }
  }
  return out
}

/** Normalizes a socket remote address into a stable per-guest identity key. */
export function normalizeIp(raw: string | undefined): string {
  if (!raw) return 'unknown'
  // Strip IPv6-mapped IPv4 prefix (::ffff:192.168.1.5 -> 192.168.1.5)
  let ip = raw.startsWith('::ffff:') ? raw.slice('::ffff:'.length) : raw
  // Treat IPv6 loopback as IPv4 loopback for consistency.
  if (ip === '::1') ip = '127.0.0.1'
  return ip
}
