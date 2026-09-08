## Analytics Tracking Checklist

Before merging, please verify:

- [ ] **New interactions registered in events.yml?** Any new user interactions (button clicks, state changes, etc.) must be added to `apps/desktop/analytics/events.yml` with proper metadata (layer, props, description)
- [ ] **Using `trackTypedEvent` for type safety?** All analytics events should use the type-safe `trackTypedEvent` function from `apps/desktop/client/src/lib/analytics.ts` to ensure compile-time validation against the event registry
- [ ] **Local verification complete?** Run `services.yaml` command `build-release-dev-keyed` (keyed release build with dev identifier) and trigger the new events locally to verify they're emitted correctly with the expected properties. Note: `pnpm -F desktop-client build` (vite build) only builds frontend assets — it does not run the app. Debug dev builds are no-op by design (environment gate, INV-3). Use the keyed release build for event verification.

---

**Note**: The event registry (`events.yml`) is the single source of truth. Codegen will fail if you reference unregistered events. See `docs/observability.md` for event naming conventions and property schemas.
