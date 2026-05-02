# Setup Wizard — Design Spec

**Date:** 2026-05-02
**Status:** Draft, pending implementation plan
**Author:** Brainstorming session, 2026-05-02

## Goal

Make the showcase site's promise true: when a fresh transcoderr
install is opened, the web UI walks the operator through configuring
a source, an optional notifier, plugins, and their first flow —
ending with a working end-to-end pipeline that fires on the next
*arr push.

The current behaviour drops the operator on an empty Dashboard with
no guidance.

## Decisions

From brainstorming, 2026-05-02:

- **Trigger:** auto-launch as a modal on first visit when no sources
  exist AND a `wizard.completed` flag isn't set in settings.
- **Flow step:** pre-fill `hevc-normalize` from a single starter
  template; operator can edit before saving.
- **Skippability:** each step has its own Skip button; closing the
  wizard sets `wizard.completed = true` so it never reappears.

## Approach

A single React modal `<SetupWizard />` mounted at the layout root.
Six views routed by an internal step state machine:

1. **Welcome.** One paragraph explaining what the wizard will do plus
   Start / "Skip wizard" buttons.
2. **Source.** Pick kind (radarr/sonarr/lidarr/webhook), fill the
   matching fields, save. Reuses the existing add-source form
   factored out of `web/src/pages/sources.tsx`.
3. **Notifier (optional).** Pick kind (discord/telegram/ntfy/webhook/
   jellyfin), fill config, save. Reuses an extracted form from
   `web/src/pages/notifiers.tsx`. Skip button.
4. **Plugins (informational).** Short paragraph explaining the plugin
   system + a button "Open Plugins" that, after Finish, navigates to
   `/plugins#browse`. No in-wizard install (avoids modal-on-modal
   complexity with `<InstallLogModal />`). Skip button.
5. **First flow.** `hevc-normalize` YAML pre-loaded into a
   read-write textarea with the visual mirror beneath. Save creates
   the flow; Skip leaves nothing.
6. **Done.** Single Finish button. Sets `wizard.completed = "true"`,
   closes the modal.

## Auto-launch trigger

```ts
const sources  = useQuery(["sources"],   api.sources.list).data ?? [];
const settings = useQuery(["settings"],  api.settings.get).data ?? {};
const open = sources.length === 0 && settings["wizard.completed"] !== "true";
```

Both Skip wizard and Finish PATCH `{"wizard.completed": "true"}` to
`/api/settings`.

## File structure

**New:**
- `web/src/components/setup-wizard.tsx` — modal shell, step state
  machine, persistence hooks. ~120 lines.
- `web/src/components/setup-wizard-steps/welcome.tsx`
- `web/src/components/setup-wizard-steps/source.tsx` — wraps the
  factored AddSourceForm.
- `web/src/components/setup-wizard-steps/notifier.tsx` — wraps the
  factored AddNotifierForm.
- `web/src/components/setup-wizard-steps/plugins.tsx` — informational
  pane.
- `web/src/components/setup-wizard-steps/flow.tsx` — template-loaded
  textarea + visual mirror.
- `web/src/components/forms/add-source.tsx` — extracted from
  `web/src/pages/sources.tsx`; takes `onCreated` callback.
- `web/src/components/forms/add-notifier.tsx` — extracted from
  `web/src/pages/notifiers.tsx`; same shape.
- `web/src/templates/hevc-normalize.yaml` — verbatim copy of
  `docs/flows/hevc-normalize.yaml`, imported via `?raw`.

**Modified:**
- `web/src/pages/sources.tsx` — uses `<AddSourceForm />` instead of
  inlining the form. Behaviour unchanged.
- `web/src/pages/notifiers.tsx` — uses `<AddNotifierForm />` ditto.
- `web/src/App.tsx` — mounts `<SetupWizard />` at the layout root so
  it overlays any page when the trigger conditions are met.

## Persistence

The `settings` table is already a free-form string → string KV
(`crates/transcoderr/src/api/settings.rs`, the existing endpoints
`GET /api/settings` and `PATCH /api/settings`). One new key,
`wizard.completed`, with value `"true"`. No backend changes, no
migration. Default missing-key behaviour maps to "not completed" so
existing installs get the wizard once on next visit.

## Visual

Modal sits over the dashboard. Two-column layout inside the modal:

- **Left rail:** numbered step list (1. Source, 2. Notifier, 3.
  Plugins, 4. Flow), with the active step highlighted and completed
  steps ticked.
- **Right pane:** the active step's content (the form or info pane).
- **Footer:** Back / Skip / Continue. Continue advances to the next
  step whether or not the current one was completed (so the operator
  isn't blocked); Back lets them review.

Reuses the existing `.modal-*` CSS classes from
`web/src/index.css` (added by `install-log-modal`). The two-column
inner layout is a small new addition: `.wizard-rail` (~180px) +
`.wizard-pane` (flex-1).

## Reuse strategy for the existing forms

The Sources and Notifiers pages each have their own inline
add-X form today. The cleanest factoring:

- Move the form JSX + state into
  `web/src/components/forms/add-source.tsx` and `add-notifier.tsx`.
- Each takes a single prop, `onCreated: (id: number) => void`, and
  invalidates the relevant React Query cache key on success.
- The Sources / Notifiers pages render `<AddSourceForm
  onCreated={(id) => qc.invalidateQueries(...)} />` — visually
  identical to today.
- The wizard renders the same forms with its own
  `onCreated` handler that advances the step.

## Tests

Two component-level tests in
`web/src/components/setup-wizard.test.tsx` (vitest):

1. Renders nothing when sources is non-empty.
2. Renders nothing when `wizard.completed === "true"`.

Manual smoke test (not automated):
- Fresh data dir + no settings → modal appears on `/dashboard`.
- Click through each step, finish; refresh browser; modal does NOT
  reappear.
- Trigger a Radarr push → the flow created via the wizard fires
  end-to-end with no other config touches.

## Out of scope

- **In-wizard plugin installs.** Would require modal-on-modal with
  the existing `<InstallLogModal />`. Deferred; operator can install
  later from `/plugins#browse`.
- **Multiple flow templates.** Just `hevc-normalize` for now per
  brainstorming. Adding more later is additive — drop more YAML
  files into `web/src/templates/` and a small picker.
- **Localisation / i18n.** Strings are English-only.
- **Tour / coachmarks** layered on top of the existing pages. The
  wizard is the only first-run guidance.
- **Re-launchable wizard** from a header button. Once
  `wizard.completed = true`, the wizard is gone; operators use the
  full pages from then on.

## Risks

- **Form factoring regression.** Refactoring sources.tsx and
  notifiers.tsx to extract forms could break existing UX. Mitigation:
  visual-identical refactor with no behaviour change; keep the
  existing tests green.
- **Wizard appears for upgrading users.** Existing installs WITH
  sources won't see it (gated on empty sources). Existing installs
  WITHOUT sources (rare) will see it once and dismiss. Acceptable.
- **Race between wizard rendering and settings load.** Show nothing
  until both queries resolve to avoid a flash. The trigger condition
  uses `?? []` and `?? {}` defaults so an in-flight query doesn't
  misfire — but those defaults match the "should show wizard"
  conditions, which would flash. Use `isLoading` from React Query to
  gate.

## Success criteria

- A first-run install lands on `/dashboard`, sees the wizard, walks
  through it, finishes; the next *arr push triggers a real
  transcode without any further configuration.
- Existing installs (with sources or `wizard.completed = true`) never
  see the wizard.
- `npm --prefix web run build` clean.
- The Sources and Notifiers pages still work exactly as before
  post-refactor.

## Branch / PR

- Branch `spec/setup-wizard` for this doc + plan.
- Implementation branch `feat/setup-wizard` from main.
- Single PR against `seanbarzilay/transcoderr` main.
