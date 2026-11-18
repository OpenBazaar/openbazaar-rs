// A throwaway offline stagenet monerod for Monero tests. Set MONEROD_EXE to
// choose the binary; `available()` is false when there is none, and those tests skip.
// Offline stagenet starts at difficulty 1, so CPU mining makes blocks in seconds.
import { spawn } from 'node:child_process'
import { existsSync } from 'node:fs'
import { mkdtemp, rm } from 'node:fs/promises'
import { createServer } from 'node:net'
import { tmpdir } from 'node:os'
import { delimiter, join } from 'node:path'

const exe = process.env.MONEROD_EXE || 'monerod'
export const available = () =>
  exe.includes('/') ? existsSync(exe) : (process.env.PATH ?? '').split(delimiter).some(dir => existsSync(join(dir, exe)))

const freePort = () =>
  new Promise((resolve, reject) => {
    const server = createServer()
    server.once('error', reject)
    server.listen(0, '127.0.0.1', () => {
      const { port } = server.address()
      server.close(() => resolve(port))
    })
  })

export async function startMonerod() {
  const dir = await mkdtemp(join(tmpdir(), 'ob-ui-monerod-'))
  const [rpcPort, p2pPort] = [await freePort(), await freePort()]
  const child = spawn(exe, [
    '--stagenet', '--offline', `--data-dir=${dir}`, '--rpc-bind-ip=127.0.0.1', `--rpc-bind-port=${rpcPort}`, `--p2p-bind-port=${p2pPort}`,
    '--non-interactive', '--no-igd', '--disable-dns-checkpoints', '--no-zmq', '--log-level=0'
  ], { stdio: 'ignore' })
  const url = `http://127.0.0.1:${rpcPort}`
  const call = async (path, body) => {
    const r = await fetch(`${url}${path}`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body ?? {}) })
    return r.json()
  }
  const height = async () => (await call('/get_info')).height
  for (let i = 0; ; i++) {
    try {
      await height()
      break
    } catch (e) {
      if (i > 150 || child.exitCode !== null) throw new Error(`monerod did not start: ${e.message}`)
      await new Promise(r => setTimeout(r, 200))
    }
  }
  return {
    url,
    height,
    /** Mines to `address` until the chain is `blocks` higher. */
    async mine(address, blocks) {
      const target = (await height()) + blocks
      const started = await call('/start_mining', { miner_address: address, threads_count: 2, do_background_mining: false, ignore_battery: true })
      if (started.status !== 'OK') throw new Error(`monerod would not mine: ${started.status}`)
      try {
        while ((await height()) < target) await new Promise(r => setTimeout(r, 250))
      } finally {
        await call('/stop_mining')
      }
    },
    async stop() {
      child.kill('SIGTERM')
      await new Promise(r => (child.exitCode === null ? child.once('exit', r) : r()))
      await rm(dir, { recursive: true, force: true })
    }
  }
}
