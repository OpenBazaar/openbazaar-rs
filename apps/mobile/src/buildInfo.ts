import { paired, parseNetworkName, type NetworkName } from './catalog.ts'

// Which build is running. The desktop shell sets __OB_SHELL_BUILD__ before the
// page loads; the node answers GET /version. Both come from the same build
// script, so a different version or commit means the app is running a node
// from another build (for example a stale sidecar).
export type BuildInfo = {
  version: string
  commit: string
  dirty: boolean
  buildDate: string
  buildKind: string
}
export type NodeBuild = BuildInfo & { network: NetworkName }

const UNKNOWN_COMMIT = 'unknown'

export function parseBuildInfo(value: unknown): BuildInfo {
  const body = value as Record<string, unknown> | null
  if (
    !body ||
    typeof body.version !== 'string' ||
    typeof body.commit !== 'string' ||
    typeof body.dirty !== 'boolean' ||
    typeof body.buildDate !== 'string' ||
    typeof body.buildKind !== 'string'
  )
    throw new Error('The build information could not be read.')
  return {
    version: body.version,
    commit: body.commit,
    dirty: body.dirty,
    buildDate: body.buildDate,
    buildKind: body.buildKind
  }
}

export function parseNodeBuild(value: unknown): NodeBuild {
  const network = parseNetworkName((value as { network?: unknown } | null)?.network)
  if (!network) throw new Error('The build information could not be read.')
  return { ...parseBuildInfo(value), network }
}

/// The desktop shell's build, or null in a browser.
export function shellBuild(
  scope: unknown = globalThis
): BuildInfo | null {
  try {
    return parseBuildInfo((scope as { __OB_SHELL_BUILD__?: unknown }).__OB_SHELL_BUILD__)
  } catch {
    return null
  }
}

export async function fetchNodeBuild(): Promise<NodeBuild> {
  return parseNodeBuild(await paired('/version'))
}

const commitLabel = (build: BuildInfo) =>
  `${build.commit}${build.dirty ? '-dirty' : ''}`

/// Why the shell and node look like different builds, or null when they
/// match. A commit of "unknown" (built without git) is not compared.
export function buildMismatch(
  shell: BuildInfo | null,
  node: BuildInfo | null
): string | null {
  if (!shell || !node) return null
  const commits =
    shell.commit !== UNKNOWN_COMMIT &&
    node.commit !== UNKNOWN_COMMIT &&
    shell.commit !== node.commit
  if (shell.version === node.version && !commits) return null
  return `This app is ${shell.version} (${commitLabel(shell)}) but its node is ${node.version} (${commitLabel(node)}). Quit OpenBazaar and reinstall it so both come from the same build.`
}

/// "OpenBazaar 0.1.0 · a1b2c3d · testnet4". The shell's build is the app's
/// when there is one; the network is the node's.
export function versionLine(
  shell: BuildInfo | null,
  node: NodeBuild | null
): string | null {
  const build = shell ?? node
  if (!build) return null
  return ['OpenBazaar ' + build.version, commitLabel(build), node?.network]
    .filter(Boolean)
    .join(' · ')
}

/// Everything known about both builds, for a bug report.
export function buildReport(
  shell: BuildInfo | null,
  node: NodeBuild | null
): string {
  const describe = (build: BuildInfo) =>
    `${build.version} ${commitLabel(build)} ${build.buildKind} build ${build.buildDate}`
  return [
    versionLine(shell, node) ?? 'OpenBazaar',
    `App: ${shell ? describe(shell) : 'browser (no desktop shell)'}`,
    `Node: ${node ? `${describe(node)} ${node.network}` : 'not reachable'}`
  ].join('\n')
}
