# Clax Spaces

Status: draft for the owner's review. Builds on per-artifact sharing
(`artifact_shares`, general access Restricted / Anyone with the link /
Public) on a hosted daemon.

## Decided

- A **Space** is a named collection of artifacts with its own members and
  its own general access. An artifact can be in **many** Spaces, or none.
  Being in a Space does not change who made the artifact or who may delete
  it.
- **Inheritance:** a Space's members get their Space role on every artifact
  in it. An artifact's own shares can add people or raise a role, never
  lower what a Space grants.
- **Roles:**
  - **View** and **Comment**: as for one artifact, on every artifact in
    the Space.
  - **Edit**: as for one artifact, on every artifact in the Space; can also
    publish new artifacts into the Space and add artifacts they may share.
  - **Admin**: Edit, plus add or remove members by GitHub `@login`, change
    roles, set the Space's general access, rename and delete the Space.
    Deleting a Space deletes no artifact.

  The creator starts as Admin; the server's owner is Admin of every Space.
- **Which Spaces a person sees:** wherever an artifact's Spaces are shown
  (the Share dialog's "In Spaces", listings, agents' `list` and `read`),
  a person sees only the Spaces they are a member of. A guest by link, a
  signed-out visitor and a spectator see none. This holds even when the
  person reached the artifact through another Space or a direct share. The
  server's owner, Admin of every Space, sees them all.
- **No cap** on how many Spaces a person creates or belongs to.
- **Ships after the submission of 2026-10-14.**

## Proposed (please confirm or change)

1. **Adding an artifact to a Space** grants every member access to it, so it
   takes both rights: the adder may share the artifact (its maker or the
   owner), and has Edit or Admin in the Space. Removing it takes either one.
2. **General access** works as for an artifact:
   - Restricted (members only);
   - Anyone with the link (`/s/<slug>`, no sign-in, view or comment);
   - Public (the same, plus listed in the public gallery).

   A Space's link or Public access reaches every artifact in it, and the
   Share dialog says so before saving.
3. **Effective access** at artifact `a` for person `p` is the highest of:
   - `p`'s own rights;
   - `a`'s share for `p`;
   - `a`'s general access;
   - for each Space containing `a`, `p`'s role there and that Space's
     general access.

   One function computes it (where per-artifact shares are resolved
   today), so the browser gate, listings, the visibility filter and the
   person's agents all agree.
4. **Slugs:** a Space has a URL slug, unique on the daemon, chosen at
   creation (lowercase letters, digits, `-`, ≤ 40). The page is
   `/s/<slug>`.
5. **UI:**
   - The gallery gets a Spaces section (yours, then shared with you) and a
     New Space button.
   - A Space page lists its artifacts with the Space's Share dialog (the
     same dialog, with Admin added).
   - An artifact's Share dialog shows "In Spaces: …" with Add to Space and
     Remove, where allowed.
6. **Agents:** MCP `publish` takes an optional `space` (slug), which needs
   Edit there; `list` takes an optional `space` filter. Local publishing to
   the public Clax (`clax push`) can name a Space too.
7. **Scope:** hosted daemons only. A local daemon has no Spaces.
8. **Storage:** a new migration (the next free number on main when this
   lands) adds:
   - `spaces` (ID, slug, name, created_by, link access, listed);
   - `space_members` (Space, person or lowercase GitHub login, role);
   - `space_artifacts` (Space, artifact, added_by, added_at), cascading on
     either side.

## Tests

- Store: membership by `@login` before and after sign-in; an artifact in two
  Spaces takes the highest role; removing it from a Space removes that
  access.
- Routes, each role both ways:
  - View can't comment, and Comment can't edit;
  - Edit can publish into the Space but can't manage members;
  - Admin can manage members but can't delete an artifact it didn't make;
  - an artifact can't be added by someone who may not share it.
- Link and Public Spaces reach their artifacts signed out; Restricted ones
  don't, and their names don't leak in listings.
- An artifact in Spaces A and B, viewed by a member of A only, through a
  direct share, or by a guest: each sees exactly the Spaces they belong to
  (A, A, none), in the dialog, in listings and through MCP.
- Browser: create a Space, add an artifact and a member, make it Public, and
  see it signed out; desktop and phone width.
