// What the API parity harness blanks out before comparing, because it differs between runs
// rather than between implementations. tests/api_electron.rs applies the same rules.
//
//   - the scratch music folder's path       → /music
//   - ISO timestamps                        → <time>
//   - the app version                       → <version>
//   - session tokens and cookie expiry      → <token>, <date>
//   - the date in the CSV export's filename → <date>
//   - durations                             → one decimal (the scanners differ below 1 ms,
//                                             and by ~50 ms on a raw ADTS stream)

const TIME = /\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z/g

export function normalize(value, root, key) {
  if (typeof value === 'string') {
    let s = value.split(root).join('/music').replace(TIME, '<time>')
    if (key === 'version') s = '<version>'
    if (key === 'set-cookie') {
      s = s.replace(/kj_session=[0-9a-f]{64}/, 'kj_session=<token>').replace(/Expires=[^;]+/, 'Expires=<date>')
    }
    if (key === 'content-disposition') s = s.replace(/\d{4}-\d{2}-\d{2}/, '<date>')
    return s
  }
  if (typeof value === 'number' && (key === 'duration')) return Math.round(value * 10) / 10
  if (Array.isArray(value)) return value.map((v) => normalize(v, root, key))
  if (value && typeof value === 'object') {
    return Object.fromEntries(Object.entries(value).map(([k, v]) => [k, normalize(v, root, k)]))
  }
  return value
}
