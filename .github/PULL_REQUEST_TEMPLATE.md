## Analytics Tracking Checklist

Before merging, please verify:

- [ ] **New interactions registered in events.yml?** Any new user interactions (button clicks, state changes, etc.) must be added to `apps/desktop/analytics/events.yml` with proper metadata (layer, props, description)
- [ ] **Using `trackTypedEvent` for type safety?** All analytics events should use the type-safe `trackTypedEvent` function from `apps/desktop/client/src/lib/analytics.ts` to ensure compile-time validation against the event registry
- [ ] **Local verification complete?** Run `pnpm -F desktop-client build` and trigger the new events locally to verify they're emitted correctly with the expected properties

---

**Note**: The event registry (`events.yml`) is the single source of truth. Codegen will fail if you reference unregistered events. See `docs/observability.md` for event naming conventions and property schemas.
