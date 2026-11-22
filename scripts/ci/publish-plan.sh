#!/usr/bin/env bash
#
# Decides what .github/workflows/publish.yml should publish, if anything.
#
# Kept as a pure function of environment variables (no network, git, or gh) so
# tests/integration.rs can table-test every branch before the workflow lands on
# main.
#
# Inputs:
#   EVENT_NAME     github.event_name: workflow_run | push | workflow_dispatch
#   REPOSITORY     github.repository, e.g. robert-crandall/rust-home-template
#   GITHUB_REF     github.ref (push and workflow_dispatch paths)
#   SHA            github.sha (push and workflow_dispatch paths)
#   RUN_EVENT      github.event.workflow_run.event
#   RUN_CONCLUSION github.event.workflow_run.conclusion
#   RUN_HEAD_SHA   github.event.workflow_run.head_sha
#   BRANCH_TIP     the current tip of the default branch
#
# Outputs (single-line key=value pairs for $GITHUB_OUTPUT):
#   publish       true | false
#   promote_main  true | false
#   ref           commit SHA to check out and build
#   image         lowercased GHCR image path
#   sha_tag       immutable sha-<40hex> tag, or empty
#   version       immutable vX.Y.Z tag, or empty
set -euo pipefail

EVENT_NAME="${EVENT_NAME:-}"
REPOSITORY="${REPOSITORY:-}"
GITHUB_REF="${GITHUB_REF:-}"
SHA="${SHA:-}"
RUN_EVENT="${RUN_EVENT:-}"
RUN_CONCLUSION="${RUN_CONCLUSION:-}"
RUN_HEAD_SHA="${RUN_HEAD_SHA:-}"
BRANCH_TIP="${BRANCH_TIP:-}"

image=$(printf '%s' "ghcr.io/${REPOSITORY}" | tr '[:upper:]' '[:lower:]')

publish=false
promote_main=false
ref=
sha_tag=
version=

case "$EVENT_NAME" in
push)
  if [[ "$GITHUB_REF" == refs/tags/v* ]]; then
    version="${GITHUB_REF#refs/tags/}"
    publish=true
    ref="$SHA"
    promote_main=false
  fi
  ;;

workflow_dispatch)
  if [[ "$GITHUB_REF" == "refs/heads/main" ]]; then
    publish=true
    promote_main=true
    ref="$SHA"
    sha_tag="sha-${SHA}"
  fi
  ;;

workflow_run)
  if [[ "$RUN_EVENT" == "push" && "$RUN_CONCLUSION" == "success" && "$RUN_HEAD_SHA" == "$BRANCH_TIP" && -n "$RUN_HEAD_SHA" ]]; then
    publish=true
    promote_main=true
    ref="$RUN_HEAD_SHA"
    sha_tag="sha-${RUN_HEAD_SHA}"
  fi
  ;;
esac

echo "publish=${publish}"
echo "promote_main=${promote_main}"
echo "ref=${ref}"
echo "image=${image}"
echo "sha_tag=${sha_tag}"
echo "version=${version}"
