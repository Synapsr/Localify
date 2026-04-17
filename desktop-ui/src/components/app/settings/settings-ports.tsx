"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";

import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { bootstrapQueries, configMutations } from "@/lib/queries";

export const SettingsPorts = () => {
  const { data } = useQuery(bootstrapQueries.read());
  const queryClient = useQueryClient();
  const updateConfig = useMutation(configMutations.update(queryClient));

  const daemon = data?.config.daemon;
  const [httpPort, setHttpPort] = useState(daemon?.httpPort ?? 80);
  const [httpsPort, setHttpsPort] = useState(daemon?.httpsPort ?? 443);
  const [dnsPort, setDnsPort] = useState(daemon?.dnsPort ?? 1053);

  if (!daemon) return null;

  async function savePorts() {
    if (!daemon) return;
    await updateConfig.mutateAsync({
      daemon: {
        ...daemon,
        httpPort,
        httpsPort,
        dnsPort,
      },
    });
  }

  return (
    <Card className="p-5">
      <div className="space-y-5">
        <div className="space-y-1">
          <h2 className="text-sm font-semibold">Ports</h2>
          <p className="text-xs text-[--color-muted]">
            Changes take effect on the next daemon restart. In system mode, saves go through the
            installed helper instead of writing directly into system files.
          </p>
        </div>
        <div className="grid gap-4 sm:grid-cols-3">
          <div className="space-y-1.5">
            <Label htmlFor="http">HTTP</Label>
            <Input
              id="http"
              type="number"
              value={httpPort}
              onChange={(e) => setHttpPort(Number(e.target.value))}
            />
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="https">HTTPS</Label>
            <Input
              id="https"
              type="number"
              value={httpsPort}
              onChange={(e) => setHttpsPort(Number(e.target.value))}
            />
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="dns">DNS</Label>
            <Input
              id="dns"
              type="number"
              value={dnsPort}
              onChange={(e) => setDnsPort(Number(e.target.value))}
            />
          </div>
        </div>
        <div className="flex justify-end">
          <Button variant="primary" loading={updateConfig.isPending} onClick={savePorts}>
            Save
          </Button>
        </div>
      </div>
    </Card>
  );
};
