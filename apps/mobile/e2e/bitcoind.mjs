// A throwaway regtest bitcoind for Lightning tests. Set BITCOIND_EXE to choose
// the binary; `available()` is false when there is none, and those tests skip.
import { spawn } from 'node:child_process'
import { existsSync } from 'node:fs'
import { mkdtemp, rm } from 'node:fs/promises'
import { createServer } from 'node:net'
import { tmpdir } from 'node:os'
import { delimiter, join } from 'node:path'

const exe = process.env.BITCOIND_EXE || 'bitcoind'
// Found on PATH, not run: `bitcoind --version` writes a settings file in the default
// data directory, and two workers doing that at once can fail.
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

export async function startBitcoind() {
  const dir = await mkdtemp(join(tmpdir(), 'ob-ui-bitcoind-'))
  const [rpcPort, p2pPort] = [await freePort(), await freePort()]
  const user = 'obtest'
  const pass = 'obtest'
  const child = spawn(exe, [
    '-regtest', `-datadir=${dir}`, `-rpcport=${rpcPort}`, `-port=${p2pPort}`, '-rpcbind=127.0.0.1', '-rpcallowip=127.0.0.1',
    `-rpcuser=${user}`, `-rpcpassword=${pass}`, '-fallbackfee=0.0002', '-txindex=1', '-listen=0', '-server=1', '-printtoconsole=0'
  ], { stdio: 'ignore' })
  const auth = `Basic ${Buffer.from(`${user}:${pass}`).toString('base64')}`
  async function rpc(method, params = [], wallet = '') {
    const r = await fetch(`http://127.0.0.1:${rpcPort}/${wallet ? `wallet/${wallet}` : ''}`, {
      method: 'POST',
      headers: { Authorization: auth, 'Content-Type': 'application/json' },
      body: JSON.stringify({ jsonrpc: '1.0', id: method, method, params })
    })
    const body = await r.json()
    if (body.error) throw new Error(`${method}: ${body.error.message}`)
    return body.result
  }
  for (let i = 0; ; i++) {
    try {
      await rpc('getblockchaininfo')
      break
    } catch (e) {
      if (i > 150 || child.exitCode !== null) throw new Error(`bitcoind did not start: ${e.message}`)
      await new Promise(r => setTimeout(r, 200))
    }
  }
  await rpc('createwallet', ['miner'])
  const minerAddress = await rpc('getnewaddress', [], 'miner')
  await rpc('generatetoaddress', [101, minerAddress])
  return {
    chainArg: `bitcoind:${user}:${pass}@127.0.0.1:${rpcPort}`,
    rpc,
    mine: (n = 1) => rpc('generatetoaddress', [n, minerAddress]),
    send: (address, btc) => rpc('sendtoaddress', [address, btc], 'miner'),
    mempool: () => rpc('getrawmempool'),
    async stop() {
      await rpc('stop').catch(() => {})
      await new Promise(r => (child.exitCode === null ? child.once('exit', r) : r()))
      await rm(dir, { recursive: true, force: true })
    }
  }
}
