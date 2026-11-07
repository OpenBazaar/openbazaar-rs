import { useMemo } from 'react'
import {
  buildMismatch,
  buildReport,
  fetchNodeBuild,
  shellBuild,
  versionLine
} from '../buildInfo'
import { useNodeResource } from '../useNodeResource'

export function useBuildInfo() {
  const shell = useMemo(() => shellBuild(), [])
  const { data: node } = useNodeResource(fetchNodeBuild, 0)
  return {
    line: versionLine(shell, node),
    report: buildReport(shell, node),
    mismatch: buildMismatch(shell, node)
  }
}
type Build = ReturnType<typeof useBuildInfo>

export function BuildLine({
  build,
  onCopied
}: {
  build: Build
  onCopied: (message: string) => void
}) {
  if (!build.line) return null
  return (
    <button
      className='ob-build-line'
      type='button'
      title='Copy build info'
      onClick={() => {
        void navigator.clipboard.writeText(build.report).then(
          () => onCopied('Build info copied'),
          () => onCopied('Build info could not be copied')
        )
      }}
    >
      {build.line}
    </button>
  )
}

export function BuildMismatchWarning({ build }: { build: Build }) {
  if (!build.mismatch) return null
  return (
    <div className='ob-network-badge ob-build-mismatch' role='alert'>
      <span className='ob-status-dot' aria-hidden='true' />
      {build.mismatch}
    </div>
  )
}
