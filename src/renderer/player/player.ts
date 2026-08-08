// Hidden player renderer: the real audio sink for the whole app.
// Receives PlayerCommands over IPC, drives an <audio> element, selects the
// output device via setSinkId, and reports playback events back to main.
import type { AudioDevice, PlayerCommand } from '@shared/types'

interface Bridge {
  onCommand: (handler: (cmd: PlayerCommand) => void) => void
  report: (event: string, payload: unknown) => void
}

const audio = document.getElementById('audio') as HTMLAudioElement & {
  setSinkId?: (id: string) => Promise<void>
}
const bridge = (window as unknown as { jukeboxPlayer?: Bridge }).jukeboxPlayer

let currentTrackId: number | null = null

function reportState(): void {
  bridge?.report('timeupdate', {
    position: audio.currentTime || 0,
    duration: Number.isFinite(audio.duration) ? audio.duration : 0,
    playing: !audio.paused,
    trackId: currentTrackId
  })
}

async function enumerate(): Promise<void> {
  try {
    const all = await navigator.mediaDevices.enumerateDevices()
    const outputs: AudioDevice[] = all
      .filter((d) => d.kind === 'audiooutput')
      .map((d, i) => ({ deviceId: d.deviceId, label: d.label || `Audio output ${i + 1}` }))
    bridge?.report('devices', outputs)
  } catch (err) {
    bridge?.report('error', `enumerate: ${String(err)}`)
  }
}

if (audio && bridge) {
  bridge.onCommand(async (cmd) => {
    switch (cmd.type) {
      case 'load':
        currentTrackId = cmd.trackId
        audio.src = `/api/stream/${cmd.trackId}`
        audio.load()
        if (cmd.autoplay) await audio.play().catch((e) => bridge.report('error', `play: ${e}`))
        break
      case 'play':
        await audio.play().catch((e) => bridge.report('error', `play: ${e}`))
        break
      case 'pause':
        audio.pause()
        break
      case 'seek':
        audio.currentTime = cmd.position
        break
      case 'volume':
        audio.volume = cmd.value
        break
      case 'setSinkId':
        if (typeof audio.setSinkId === 'function') {
          await audio.setSinkId(cmd.deviceId || '').catch((e) =>
            bridge.report('error', `setSinkId: ${e}`)
          )
        }
        break
      case 'enumerate':
        await enumerate()
        break
    }
  })

  audio.addEventListener('timeupdate', reportState)
  audio.addEventListener('play', reportState)
  audio.addEventListener('pause', reportState)
  audio.addEventListener('durationchange', reportState)
  audio.addEventListener('ended', () => bridge.report('ended', {}))
  audio.addEventListener('error', () =>
    bridge.report('error', `audio element error (code ${audio.error?.code ?? '?'})`)
  )
  navigator.mediaDevices.addEventListener('devicechange', () => void enumerate())

  bridge.report('ready', { at: Date.now() })
  void enumerate()
  console.log('[player] renderer ready')
} else {
  console.warn('[player] audio element or bridge missing')
}
