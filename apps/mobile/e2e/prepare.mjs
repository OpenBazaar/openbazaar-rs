// Builds what the real-node tests run: ob-node (unless OB_NODE_BIN is set) and
// the web export when it is older than the app source.
import { spawnSync } from 'node:child_process'
import { existsSync, readdirSync, statSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { app } from './nodes.mjs'

function run(command, args, cwd) {
  console.log(`> ${command} ${args.join(' ')}`)
  const r = spawnSync(command, args, { cwd, stdio: 'inherit' })
  if (r.status !== 0) process.exit(r.status ?? 1)
}

function newest(path) {
  if (!existsSync(path)) return 0
  const stat = statSync(path)
  if (!stat.isDirectory()) return stat.mtimeMs
  return Math.max(0, ...readdirSync(path).map(name => newest(join(path, name))))
}

if (!process.env.OB_NODE_BIN) run('cargo', ['build', '-p', 'ob-node', '--locked'], resolve(app, '../..'))

const built = newest(join(app, 'dist', 'index.html'))
const source = Math.max(...['src', 'App.tsx', 'index.ts', 'app.json', 'package-lock.json', 'public'].map(p => newest(join(app, p))))
if (process.env.OB_E2E_SKIP_WEB_BUILD !== '1' && built < source) run('npm', ['run', 'build'], app)
