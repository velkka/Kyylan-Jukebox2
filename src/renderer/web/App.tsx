import { useCallback, useEffect, useState } from 'react'
import type { PublicConfig } from '@shared/types'
import { getConfig } from './api'
import FirstRunSetup from './FirstRunSetup'
import { JukeboxProvider } from './JukeboxContext'
import Home from './components/Home'

export default function App(): JSX.Element {
  const [config, setConfig] = useState<PublicConfig | null>(null)
  const [error, setError] = useState<string | null>(null)

  const load = useCallback(() => {
    setError(null)
    getConfig()
      .then(setConfig)
      .catch((e: unknown) => setError(String(e)))
  }, [])

  useEffect(load, [load])

  if (error) {
    return (
      <Centered>
        <p className="text-red-400">Server unreachable: {error}</p>
      </Centered>
    )
  }

  if (!config) {
    return (
      <Centered>
        <p className="text-white/50">Connecting to server…</p>
      </Centered>
    )
  }

  if (!config.configured) {
    return <FirstRunSetup config={config} onDone={load} />
  }

  return (
    <JukeboxProvider>
      <Home />
    </JukeboxProvider>
  )
}

function Centered({ children }: { children: React.ReactNode }): JSX.Element {
  return (
    <div className="min-h-full flex items-center justify-center p-8 text-center">{children}</div>
  )
}
