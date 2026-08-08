// electron-builder afterPack hook.
// If the packed macOS app has no real (Developer ID) signature, apply a deep
// ad-hoc signature so unsigned local builds still launch on Apple Silicon.
// When a Developer ID cert is present, electron-builder signs first and this
// hook detects that signature and leaves it untouched.
const { spawnSync, execFileSync } = require('node:child_process')
const { join } = require('node:path')

exports.default = async function afterPack(context) {
  if (context.electronPlatformName !== 'darwin') return
  const appPath = join(context.appOutDir, `${context.packager.appInfo.productFilename}.app`)

  const probe = spawnSync('codesign', ['-dvv', appPath], { encoding: 'utf8' })
  const info = `${probe.stdout || ''}${probe.stderr || ''}`
  if (/Authority=Developer ID/.test(info)) {
    console.log('  • afterPack: Developer ID signature present — leaving as-is')
    return
  }

  const entitlements = join(context.packager.projectDir, 'build', 'entitlements.mac.plist')
  try {
    execFileSync('xattr', ['-cr', appPath], { stdio: 'ignore' })
    execFileSync(
      'codesign',
      ['--force', '--deep', '--sign', '-', '--options', 'runtime', '--entitlements', entitlements, appPath],
      { stdio: 'ignore' }
    )
    console.log('  • afterPack: applied ad-hoc signature (unsigned build, runs locally)')
  } catch (err) {
    console.warn('  • afterPack: ad-hoc signing failed:', err.message)
  }
}
