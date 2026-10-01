declare namespace Cloudflare {
  interface Env {
    /// The migrations, read by vitest.config.ts for test/migrate.ts.
    TEST_MIGRATIONS: import("cloudflare:test").D1Migration[];
  }
}
