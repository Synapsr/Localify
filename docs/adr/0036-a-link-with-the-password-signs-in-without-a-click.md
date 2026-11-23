# A Link With the Password Signs In Without a Click

A link that carries a public URL's password used to wait for one click on a browser's first visit to the host: ADR 0026 put the warning page before it, so an attacker could not protect a URL and send visitors straight to a fake page. In practice the person opening these links is almost always the owner, or someone the owner works with, opening a page an agent prepared on their phone or laptop. That click was the one step left between the link and the page. This amends ADR 0026.

**The link signs in and acknowledges the warning together.** `https://<host>/__localify/access#<password>` posts the password by itself, with the warning's form token, whether or not the browser has seen the warning. A right password sets both cookies and opens the app; a wrong one acknowledges nothing and shows the warning with a password field. The warning still comes first for every other visitor: an open URL, a link without the password, a URL typed by hand.

**The link lands on a page.** `?next=<path>` on the access path names the page to open after signing in, on that same host. `localify public-url link <page>`, and the MCP tool `public_url_link`, turn a local page such as `https://admin.louez.localify/orders/42` into that link on its public host, so an agent can put a person on the exact page it means.

**The pages say less.** The warning shows the host, the owner's GitHub login and one line asking to trust the sender; the rest of what ADR 0026 lists sits behind a `?` next to the heading. The password page shows the host and the field. Every Worker page uses Localify's look.

**The accepted risk.** Someone who protects a URL can send a link that opens a fake page with no warning. We accept it for the reasons ADR 0026 accepted shared cookies: every owner has paid with a card, is known by a GitHub account, and can be banned; every page the Worker serves links to `abuse@localify.dev`; and the password still decides who gets in. If phishing through links shows up, the link goes back to waiting for one click.
