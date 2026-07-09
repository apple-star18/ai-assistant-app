import { useEffect, useState } from 'react';

import { env } from './config/env';
import { getAppHealth } from './lib/tauri/client';
import type { AppHealth } from './lib/tauri/contracts';

export function App() {
  const [health, setHealth] = useState<AppHealth | null>(null);

  useEffect(() => {
    void getAppHealth().then(setHealth);
  }, []);

  return (
    <main className="app-shell">
      <section className="status-panel" aria-labelledby="app-title">
        <p className="eyebrow">{env.appEnv}</p>
        <h1 id="app-title">AI Assistant</h1>
        <dl>
          <div>
            <dt>Frontend</dt>
            <dd>React + TypeScript + Vite</dd>
          </div>
          <div>
            <dt>Backend</dt>
            <dd>{health ? `Rust/Tauri ${health.version}` : 'Connecting...'}</dd>
          </div>
          <div>
            <dt>Status</dt>
            <dd>{health?.status ?? 'pending'}</dd>
          </div>
        </dl>
      </section>
    </main>
  );
}
