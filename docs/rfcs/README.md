# RFCs

An RFC holds the design of a change before code. Bugs and small fixes go to [GitHub issues of softcery/scswarm](https://github.com/softcery/scswarm/issues) instead.

An RFC file on `main` is accepted and not yet built. Rejected RFCs never merge. Built RFCs get deleted.

## Propose

1. Create branch `rfc/<slug>` from `main`.
2. Copy [0000-template.md](0000-template.md) to `docs/rfcs/0000-<slug>.md`.
3. Fill every section. Give evidence as file and line, numbers, dates.
4. Open a pull request titled `docs(rfc): <description>`.
5. Rename the file to `<PR number>-<slug>.md`, zero-padded to 4 digits, and push.
6. Revise on review comments.

## Accept

1. Merge the pull request.

## Build

1. Reference the RFC in the implementing pull request.
2. In that pull request, move each lasting rule into `docs/refs/` and each contract change into `docs/spec/`.
3. In that pull request, delete the RFC file.

## Reject

1. Close the pull request with the reason.
