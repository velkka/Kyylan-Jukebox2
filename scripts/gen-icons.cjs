// Generates app + tray icons from the real logo (build/logo-source.png).
//  - build/icon.png  (512) : logo composited on the purple badge
//  - build/tray.png  (32)  : adaptive template silhouette for the macOS menubar
//  - build/tray@2x.png (64)
const zlib = require('node:zlib')
const { readFileSync, writeFileSync, mkdirSync, existsSync } = require('node:fs')
const { join } = require('node:path')

// ---- PNG codec ---------------------------------------------------------------
const CRC = (() => {
  const t = new Uint32Array(256)
  for (let n = 0; n < 256; n++) {
    let c = n
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1
    t[n] = c >>> 0
  }
  return (buf) => {
    let c = 0xffffffff
    for (let i = 0; i < buf.length; i++) c = t[(c ^ buf[i]) & 0xff] ^ (c >>> 8)
    return (c ^ 0xffffffff) >>> 0
  }
})()
function chunk(type, data) {
  const len = Buffer.alloc(4)
  len.writeUInt32BE(data.length, 0)
  const td = Buffer.concat([Buffer.from(type, 'latin1'), data])
  const crc = Buffer.alloc(4)
  crc.writeUInt32BE(CRC(td), 0)
  return Buffer.concat([len, td, crc])
}
function encodePng(size, pixels) {
  const ihdr = Buffer.alloc(13)
  ihdr.writeUInt32BE(size, 0)
  ihdr.writeUInt32BE(size, 4)
  ihdr[8] = 8
  ihdr[9] = 6
  const raw = Buffer.alloc(size * (size * 4 + 1))
  for (let y = 0; y < size; y++) {
    raw[y * (size * 4 + 1)] = 0
    pixels.copy(raw, y * (size * 4 + 1) + 1, y * size * 4, (y + 1) * size * 4)
  }
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', ihdr),
    chunk('IDAT', zlib.deflateSync(raw, { level: 9 })),
    chunk('IEND', Buffer.alloc(0))
  ])
}

// Decode an 8-bit RGBA, non-interlaced PNG (color type 6).
function decodePng(buf) {
  const width = buf.readUInt32BE(16)
  const height = buf.readUInt32BE(20)
  if (buf[24] !== 8 || buf[25] !== 6) throw new Error('expected 8-bit RGBA PNG')
  let i = 8
  const idat = []
  while (i < buf.length) {
    const len = buf.readUInt32BE(i)
    const type = buf.toString('latin1', i + 4, i + 8)
    if (type === 'IDAT') idat.push(buf.subarray(i + 8, i + 8 + len))
    if (type === 'IEND') break
    i += 12 + len
  }
  const inflated = zlib.inflateSync(Buffer.concat(idat))
  const stride = width * 4
  const out = Buffer.alloc(height * stride)
  const paeth = (a, b, c) => {
    const p = a + b - c
    const pa = Math.abs(p - a)
    const pb = Math.abs(p - b)
    const pc = Math.abs(p - c)
    return pa <= pb && pa <= pc ? a : pb <= pc ? b : c
  }
  for (let y = 0; y < height; y++) {
    const filter = inflated[y * (stride + 1)]
    const rowIn = y * (stride + 1) + 1
    for (let x = 0; x < stride; x++) {
      const raw = inflated[rowIn + x]
      const a = x >= 4 ? out[y * stride + x - 4] : 0
      const b = y > 0 ? out[(y - 1) * stride + x] : 0
      const c = x >= 4 && y > 0 ? out[(y - 1) * stride + x - 4] : 0
      let v = raw
      if (filter === 1) v = raw + a
      else if (filter === 2) v = raw + b
      else if (filter === 3) v = raw + ((a + b) >> 1)
      else if (filter === 4) v = raw + paeth(a, b, c)
      out[y * stride + x] = v & 0xff
    }
  }
  return { width, height, data: out }
}

// Bilinear sample → premultiplied RGBA (0..255).
function sample(img, x, y) {
  const { width, height, data } = img
  x = Math.max(0, Math.min(width - 1.001, x))
  y = Math.max(0, Math.min(height - 1.001, y))
  const x0 = Math.floor(x)
  const y0 = Math.floor(y)
  const fx = x - x0
  const fy = y - y0
  const at = (xx, yy, k) => {
    const idx = (yy * width + xx) * 4
    const al = data[idx + 3]
    if (k === 3) return al
    return (data[idx + k] * al) / 255 // premultiplied
  }
  const out = [0, 0, 0, 0]
  for (let k = 0; k < 4; k++) {
    const top = at(x0, y0, k) * (1 - fx) + at(x0 + 1, y0, k) * fx
    const bot = at(x0, y0 + 1, k) * (1 - fx) + at(x0 + 1, y0 + 1, k) * fx
    out[k] = top * (1 - fy) + bot * fy
  }
  return out // premultiplied
}

const rrect = (x, y, r) => {
  const dx = Math.max(Math.abs(x - 0.5) - (0.5 - r), 0)
  const dy = Math.max(Math.abs(y - 0.5) - (0.5 - r), 0)
  return Math.hypot(dx, dy) <= r
}

function renderApp(size, logo) {
  const px = Buffer.alloc(size * size * 4)
  const SS = 3
  const boxW = size * 0.84
  const boxH = (boxW * logo.height) / logo.width
  const offX = (size - boxW) / 2
  const offY = (size - boxH) / 2
  for (let py = 0; py < size; py++) {
    for (let pxx = 0; pxx < size; pxx++) {
      let badge = 0
      let pr = 0, pg = 0, pb = 0, pa = 0
      for (let sy = 0; sy < SS; sy++) {
        for (let sx = 0; sx < SS; sx++) {
          const fxp = pxx + (sx + 0.5) / SS
          const fyp = py + (sy + 0.5) / SS
          if (rrect(fxp / size, fyp / size, 0.24)) badge++
          const lx = ((fxp - offX) / boxW) * logo.width
          const ly = ((fyp - offY) / boxH) * logo.height
          if (lx >= 0 && lx < logo.width && ly >= 0 && ly < logo.height) {
            const s = sample(logo, lx, ly)
            pr += s[0]
            pg += s[1]
            pb += s[2]
            pa += s[3]
          }
        }
      }
      const tot = SS * SS
      const la = pa / tot / 255 // logo coverage 0..1
      // unpremultiply logo color
      const lr = pa > 0 ? (pr / tot / (pa / tot)) * 255 : 0
      const lg = pa > 0 ? (pg / tot / (pa / tot)) * 255 : 0
      const lb = pa > 0 ? (pb / tot / (pa / tot)) * 255 : 0
      const g = py / size
      const br = 0xc0 * (1 - g) + 0x7a * g
      const bg = 0x4c * (1 - g) + 0x2b * g
      const bb = 0xff * (1 - g) + 0xd6 * g
      const i = (py * size + pxx) * 4
      px[i] = Math.round(br * (1 - la) + lr * la)
      px[i + 1] = Math.round(bg * (1 - la) + lg * la)
      px[i + 2] = Math.round(bb * (1 - la) + lb * la)
      px[i + 3] = Math.round(255 * (badge / tot))
    }
  }
  return px
}

// Template silhouette: opaque where the logo is dark (eye outline + pupils),
// transparent over the white eyeballs and background. macOS tints it to match.
function renderTrayTemplate(size, logo) {
  const px = Buffer.alloc(size * size * 4)
  const SS = 3
  const boxW = size * 0.94
  const boxH = (boxW * logo.height) / logo.width
  const offX = (size - boxW) / 2
  const offY = (size - boxH) / 2
  for (let py = 0; py < size; py++) {
    for (let pxx = 0; pxx < size; pxx++) {
      let cover = 0
      for (let sy = 0; sy < SS; sy++) {
        for (let sx = 0; sx < SS; sx++) {
          const lx = ((pxx + (sx + 0.5) / SS - offX) / boxW) * logo.width
          const ly = ((py + (sy + 0.5) / SS - offY) / boxH) * logo.height
          if (lx >= 0 && lx < logo.width && ly >= 0 && ly < logo.height) {
            const s = sample(logo, lx, ly)
            const a = s[3] / 255
            if (a > 0) {
              const lum = a > 0 ? (s[0] + s[1] + s[2]) / 3 / a / 255 : 0
              cover += a * (1 - lum) // dark & opaque → 1
            }
          }
        }
      }
      const i = (py * size + pxx) * 4
      px[i] = 0
      px[i + 1] = 0
      px[i + 2] = 0
      px[i + 3] = Math.round(255 * (cover / (SS * SS)))
    }
  }
  return px
}

const out = join(__dirname, '..', 'build')
mkdirSync(out, { recursive: true })
const src = join(out, 'logo-source.png')
if (!existsSync(src)) {
  console.error('[gen-icons] build/logo-source.png not found — skipping.')
  process.exit(0)
}
const logo = decodePng(readFileSync(src))
writeFileSync(join(out, 'icon.png'), encodePng(512, renderApp(512, logo)))
writeFileSync(join(out, 'tray.png'), encodePng(32, renderTrayTemplate(32, logo)))
writeFileSync(join(out, 'tray@2x.png'), encodePng(64, renderTrayTemplate(64, logo)))
console.log('wrote build/icon.png, build/tray.png, build/tray@2x.png from logo-source.png')
