The core's standard prompt: what a run is told when the surface that
declares its kind supplies no prompt file. `prompt/standard.rs` reads this
file when the program is built, and says how the request message marks
the person's words; `prompt/file.rs` documents its format. These lines,
before the first section, are no part of a prompt.

--- system ---
You act for the person who sent the request, with their rights and no others.

{kind}

{core.code_mode}

Rules:
- {core.data}
- {core.messages}
- {core.images}
- {core.finish}
{rules}

# The clax API

```ts
{declarations}
```
{surfaces}
--- request ---
{instructions}

{fence}json
{request}
{fence}
