import { useState } from "react";
import { Button } from "../components/ui/button";
import { Card } from "../components/ui/card";
import { EmptyState } from "../components/ui/empty-state";
import { DomainCard } from "../components/domain-card";
import { AddDomainForm } from "../components/add-domain-form";
import { useBootstrap } from "../lib/queries";

export function DomainsScreen() {
  const { data } = useBootstrap();
  const [adding, setAdding] = useState(false);

  const domains = data?.domains ?? [];
  const suffix = data?.environment.fixedDomainSuffix ?? "";

  return (
    <section className="space-y-6">
      <div className="flex items-end justify-between">
        <div className="space-y-1">
          <h1 className="text-xl font-semibold tracking-tight">Domains</h1>
          <p className="text-sm text-[--color-muted]">
            Register local domains and route requests to your apps.
          </p>
        </div>
        {!adding && domains.length > 0 && (
          <Button variant="primary" onClick={() => setAdding(true)}>
            New domain
          </Button>
        )}
      </div>

      {adding && (
        <Card className="p-5">
          <AddDomainForm
            suffix={suffix}
            onClose={() => setAdding(false)}
          />
        </Card>
      )}

      {domains.length === 0 && !adding && (
        <EmptyState
          title="No domains yet"
          description={`Register your first ${suffix || ".roxy"} domain and point it at a local port or directory.`}
          action={
            <Button variant="primary" onClick={() => setAdding(true)}>
              Add your first domain
            </Button>
          }
        />
      )}

      {domains.length > 0 && (
        <ul className="space-y-3">
          {domains.map((domain) => (
            <li key={domain.registration.pattern}>
              <DomainCard domain={domain} />
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
