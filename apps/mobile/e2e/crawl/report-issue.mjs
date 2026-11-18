// Opens, or comments on, the one open "ui-crawl" issue with a nightly crawl's findings.
import { execFileSync } from 'node:child_process'
import { readdirSync, readFileSync, statSync } from 'node:fs'
import { join } from 'node:path'

function find(dir, name) {
  for (const entry of readdirSync(dir)) {
    const path = join(dir, entry)
    if (statSync(path).isDirectory()) {
      const found = find(path, name)
      if (found) return found
    } else if (entry === name) return path
  }
  return null
}

const gh = (...args) => execFileSync('gh', args, { encoding: 'utf8' }).trim()
const file = find('test-results', 'findings.json')
const report = file ? JSON.parse(readFileSync(file, 'utf8')) : null
const lines = report
  ? [
      ...report.findings.map(f => `- **${f.node}**: ${f.kind} on _${f.screen}_ › \`${f.control}\` (step ${f.step}): ${f.detail}`),
      ...report.problems.map(p => `- ${p}`)
    ]
  : ['- No crawl findings were written: the Lightning tests or the crawl setup failed. See the run.']
if (!lines.length) lines.push('- The crawl found nothing; the Lightning browser tests failed. See the run.')
const body = [
  `Nightly UI run failed. Crawl seed \`${report?.seed ?? 'unknown'}\`; replay with \`OB_CRAWL_SEED=${report?.seed ?? ''} npm run test:crawl\` in apps/mobile.`,
  '',
  ...lines,
  '',
  `Run, with screenshots, traces and node logs: ${process.env.RUN_URL}`
].join('\n')

gh('label', 'create', 'ui-crawl', '--color', 'd93f0b', '--description', 'Found by the nightly UI crawl', '--force')
const open = gh('issue', 'list', '--label', 'ui-crawl', '--state', 'open', '--json', 'number', '--jq', '.[0].number')
if (open) gh('issue', 'comment', open, '--body', body)
else gh('issue', 'create', '--title', 'Nightly UI crawl findings', '--label', 'ui-crawl', '--body', body)
