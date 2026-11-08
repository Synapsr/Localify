// `cloudflare:workers` for the tests, which run in Node: the Durable Object
// base class, which only keeps its state and environment.

export class DurableObject<E = Cloudflare.Env> {
  constructor(
    protected readonly ctx: DurableObjectState,
    protected readonly env: E,
  ) {}
}
