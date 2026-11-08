// Counting a run's D1 queries. A Worker invocation may send at most 1,000
// on the Workers Paid plan, so the sweep spends them from a budget, as it
// does its Cloudflare calls, and keeps what its revocations need.

import { CallBudget } from "./cloudflare";

/**
 * `db` with every query it runs taken from `budget`. A batch counts as one,
 * as D1 counts it against the invocation's limit. It never refuses one: the
 * run checks `budget.has` before each piece of work, and D1 itself refuses
 * past its limit.
 */
export function meteredDb(db: D1Database, budget: CallBudget): D1Database {
  const spend = (count: number) => {
    budget.left -= count;
  };
  return {
    prepare: (sql: string) => metered(db.prepare(sql), spend),
    batch: (statements: D1PreparedStatement[]) => {
      spend(1);
      return db.batch(statements.map(unwrap));
    },
    exec: (sql: string) => {
      spend(1);
      return db.exec(sql);
    },
  } as unknown as D1Database;
}

const INNER = Symbol("inner");

type Metered = D1PreparedStatement & { [INNER]: D1PreparedStatement };

function metered(statement: D1PreparedStatement, spend: (count: number) => void): Metered {
  const counted =
    <T>(run: () => Promise<T>) =>
    (): Promise<T> => {
      spend(1);
      return run();
    };
  return {
    [INNER]: statement,
    bind: (...values: unknown[]) => metered(statement.bind(...values), spend),
    first: (column?: string) => {
      spend(1);
      return column === undefined ? statement.first() : statement.first(column);
    },
    all: counted(() => statement.all()),
    run: counted(() => statement.run()),
    raw: (options?: { columnNames?: boolean }) => {
      spend(1);
      return statement.raw(options as { columnNames: true });
    },
  } as unknown as Metered;
}

function unwrap(statement: D1PreparedStatement): D1PreparedStatement {
  return (statement as Partial<Metered>)[INNER] ?? statement;
}
