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

