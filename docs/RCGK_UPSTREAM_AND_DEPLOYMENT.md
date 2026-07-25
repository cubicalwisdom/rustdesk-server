# RCGK upstream and deployment workflow

## Branch and remote policy

- `upstream` fetches from `https://github.com/rustdesk/rustdesk-server.git` and has no usable push URL.
- `origin` is the writable `cubicalwisdom/rustdesk-server` fork.
- `master` is an unmodified mirror of `upstream/master`. Never add RCGK commits to it.
- `rcgk-server` starts from an official release tag and contains reviewed RCGK changes.
- Use `feature/<name>` for customization work and `upgrade/<version>` for upstream release integration.

The scheduled `Sync Upstream Master` workflow fast-forwards only. It fails instead of creating a merge commit when the fork diverges from official `master`.

## Integrating an official release

Fetch official tags and create an upgrade branch from the deployed RCGK branch:

```powershell
git fetch --prune --tags upstream
git switch rcgk-server
git pull --ff-only origin rcgk-server
git switch -c upgrade/1.1.17
git merge --no-ff 1.1.17
```

Resolve conflicts on the upgrade branch, run the verification workflow, and merge the upgrade branch into `rcgk-server`. Do not rebase or force-push the shared `rcgk-server` branch.

## Image and production policy

- Build from an exact commit on `rcgk-server`.
- Produce `linux/arm64` binaries and an image for the OCI host.
- Tag images with the upstream version and RCGK revision, for example `1.1.17-rcgk.1`.
- Record and deploy the immutable image digest. Never deploy a floating `latest` tag.
- Keep the OCI Compose file and persistent RustDesk state outside the source repository.
- Never commit server credentials, registry credentials, private identity keys, SQLite state, or unrestricted logs.
- Preserve `/opt/stacks/data:/root` so existing clients retain the server identity and database.

## Production upgrade sequence

1. Verify the image architecture, `hbbs` and `hbbr` entrypoints, CLI compatibility, and immutable digest.
2. Keep the live containers running while pulling and inspecting the new image.
3. Back up `/opt/stacks/rustdesk-compose.yml` and a consistent copy of `/opt/stacks/data` during the approved maintenance window.
4. Replace only the RustDesk `hbbs` and `hbbr` containers.
5. Verify DNS, TCP 21115-21117, UDP 21116, identity continuity, database access, registration, and relay behavior.
6. If verification fails, restore the old Compose file, old image digest, and complete pre-cutover state directory.

## RCGK custom IDs

The RCGK `hbbs` build accepts the stock RustDesk client's official Change ID request. It renames an existing peer only when the request's old ID and machine UUID match the stored peer, and it rejects names already owned by another peer.

Custom IDs must use the official client format:

- 6-16 characters.
- Start with an ASCII letter.
- Use only letters, digits, `_`, or `-` after the first character.
- Do not use spaces, dots, or `@`.

On an installed client, unlock **Settings > Security** and use **Change ID**. On Windows, the equivalent administrator PowerShell command is:

```powershell
& "$env:ProgramFiles\RustDesk\rustdesk.exe" --set-id "farm-pc01" | Out-String
```

The client must be online against the RCGK ID server while changing its ID. The old numeric ID stops resolving after a successful rename. Device UUID, public key, password, approval settings, and the remaining peer database fields are preserved. Back up a pilot client's configuration before its first rename because the official Change ID form does not accept a numeric ID for a direct rename back.

## Current deployment record

- Custom-ID cutover date: 2026-07-26 (Asia/Calcutta; 2026-07-25 UTC)
- Source branch: `rcgk-server`
- Source commit: `ed643bf8b4e22818a2aab464d39b9e395721ec06`
- Upstream baseline: `1.1.16` (`73523b31cfd25d77dee862e6fc9f5e1fb5e485ef`)
- Release tag: `1.1.16-rcgk.3-arm64`
- Deployed image: `ghcr.io/cubicalwisdom/rustdesk-server@sha256:44c499c6e6fd0ddf181ae7a0834bd70bf74d111118729011a5a270318c7a0088`
- Previous image: `ghcr.io/cubicalwisdom/rustdesk-server@sha256:c8abc1e853794b9698ac650ca3ce27dd39a72c1ee6a009815e3e0db8b813438a`
- Pre-cutover OCI backup: `/opt/stacks/backups/rustdesk-20260725T234651Z-custom-id`
- Compose path: `/opt/stacks/rustdesk-compose.yml`
- Persistent state: `/opt/stacks/data:/root`
- Verification: both containers running with zero restarts; required TCP/UDP listeners present; external TCP 21115-21117 reachable; identity hash unchanged; live and backup SQLite `quick_check` results `ok` with three peer rows each.

The current RCGK image is compatible with the existing 1.1.16 deployment and adds only the stock-client Change ID handler described above. Add future server features as isolated, tested commits on `feature/<name>` branches.
