---
type: fixed
---

## Pipe Localify's output without a crash

Commands like `localify status | head -1` now stop quietly when the output is closed,
like other command line tools. Before, they crashed and left a crash report behind.
