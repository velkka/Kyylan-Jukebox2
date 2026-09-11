import { hostname as ownHostname, networkInterfaces } from 'node:os'
import { promises as dns } from 'node:dns'

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

// Reverse lookups are slow and often fail on a LAN, so remember each answer
// (including the IP fallback) for the life of the process.
const hostnameCache = new Map<string, string>()

/** Just the device part: "Velkkas-iPhone.local." -> "Velkkas-iPhone". */
function shortHostname(name: string): string {
  return name.replace(/\.$/, '').split('.')[0] || name
}

/**
 * Best-effort display name for a guest: the hostname their device reports,
 * falling back to the raw IP when the network can't tell us.
 */
export async function resolveHostname(ip: string): Promise<string> {
  const cached = hostnameCache.get(ip)
  if (cached) return cached

  let name = ip
  if (ip === '127.0.0.1') {
    name = shortHostname(ownHostname())
  } else {
    try {
      // lookupService() goes through the OS resolver, so mDNS/.local names on a
      // LAN resolve — dns.reverse() talks to DNS directly and misses them.
      const { hostname } = await dns.lookupService(ip, 0)
      if (hostname && hostname !== ip) name = shortHostname(hostname)
    } catch {
      // No PTR/mDNS record — the IP is a reasonable stand-in.
    }
  }
  hostnameCache.set(ip, name)
  return name
}
