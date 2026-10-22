# Security

## Report a vulnerability

Report through GitHub private vulnerability reporting: [open a draft advisory on softcery/scswarm](https://github.com/softcery/scswarm/security/advisories/new). Do not open a public issue.

Only the latest commit on `main` gets fixes. There are no releases yet.

## Trust model

scswarm runs agent turns and `run` steps with nobody at the keyboard. Treat everything they produce as untrusted code.

The slot container is the only boundary. Source: `src/podman.rs`.

| Path or resource | Access inside the container |
|---|---|
| slot worktree | read-write |
| bare repo (gitdir) | read-only |
| each `mounts` entry of the host file | read-write |
| each `devices` entry of the host file | passed as `--device` |
| each `env` name of the host file | value visible to every process |
| network | podman default: outbound open |

Facts that follow from the table:

- The container runs rootless with `--userns=keep-id`. Files it writes on a bind belong to your host user.
- An agent can read every forwarded `env` value and send it out over the network. Forward only credentials scoped to the agent.
- A `mounts` entry shared by all slots, for example a cargo registry, lets one row change what the next row builds with.
- The agent never writes git refs. The host commits the worktree. The gitdir mounts read-only, so the agent cannot add a hook or change git config.
- Agent permission flags, for example `--dangerously-skip-permissions`, come from `exec` in your `scswarm.yaml`. scswarm adds none.
- Review each `swarm/<id>` branch before you merge it. A merge runs nothing, but your next build runs the agent's code on the host.

## Out of scope

- An agent CLI or model doing what your prompt or `exec` flags allow.
- Escapes from podman or the kernel. Report those to podman or your distribution.
