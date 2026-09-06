import type { GuardDecision } from './guard.js'
import type { ValidationIssue } from './schema.js'

/** Optimistic-concurrency failure: the entry changed since it was read. */
export class ConflictError extends Error {
  constructor(message: string) {
    super(message)
    this.name = 'ConflictError'
  }
}

/** The requested id does not exist in any layer. */
export class EntryNotFoundError extends Error {
  readonly id: string

  constructor(id: string) {
    super(`entry not found: ${id}`)
    this.name = 'EntryNotFoundError'
    this.id = id
  }
}

/** The ownership guard blocked the operation (fail-closed). */
export class GuardDeniedError extends Error {
  readonly decision: GuardDecision

  constructor(decision: GuardDecision) {
    super(decision.reason)
    this.name = 'GuardDeniedError'
    this.decision = decision
  }
}

/** The entry failed schema validation. */
export class ValidationError extends Error {
  readonly issues: ValidationIssue[]

  constructor(issues: ValidationIssue[]) {
    super(`invalid entry: ${issues.map((issue) => `${issue.field}: ${issue.message}`).join('; ')}`)
    this.name = 'ValidationError'
    this.issues = issues
  }
}
