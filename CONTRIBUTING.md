# Contributing to Clax

Thank you for helping make Clax better. Bug reports, ideas and pull requests
are all welcome.

## Reporting a problem or an idea

Open an issue at <https://github.com/empathic/clax/issues>. For a bug, say
what you did, what you expected and what happened, and include the output
of `clax doctor` (and `clax doctor --agent <claude|codex|pi|grok>` for a
problem with one harness).

## Sending a change

1. **Sign the Contributor License Agreement, once.** Before Empathic can
   merge your first pull request, read the
   [Individual Contributor License Agreement](CLA.md), fill in its
   signature block, sign it, and email it to Alex Kesling at
   <alex@empathic.dev> (a scanned, photographed or electronically signed
   copy is fine). You keep the copyright in your work; the agreement grants
   Empathic the license it needs to distribute your contribution as part of
   Clax. If your employer has rights in what you write, get its permission
   first (section 4 of the agreement). Signed agreements are kept by
   Empathic and are not published; the
   [Privacy Policy](docs/legal/privacy.md) covers the details they carry.
2. **Build and test it.** [Developing Clax](docs/usage.md#developing-clax)
   covers building from source, the tests and the checks. Run `just check`
   before you open the pull request.
3. **Open a pull request** against `main`, describing what the change does
   and why. Say in it that you have sent the signed agreement.

Coding agents working in this repository follow [AGENTS.md](AGENTS.md) as
well.

## License

By contributing, you agree that your contributions are licensed to Empathic
under the [Contributor License Agreement](CLA.md), and Clax distributes them
under the licenses in [LICENSE](LICENSE).
