"use client";

import { RefreshIcon } from "@/components/icons";
import { InputGroupButton } from "@/components/ui/input-group";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { withFieldGroup } from "@/hooks/form/form";

export const DomainTargetFields = withFieldGroup({
  defaultValues: {
    domain: "",
    target: "",
  },
  props: {
    domainSuffix: "",
    onFillPort: () => {},
    isFindingPort: false,
  },
  render: function Render({ group, domainSuffix, onFillPort, isFindingPort }) {
    return (
      <div className="grid gap-4 sm:grid-cols-2">
        <group.AppField name="domain">
          {(field) => (
            <field.TextField
              label="Domain"
              placeholder="my-app"
              inputSuffix={domainSuffix}
            />
          )}
        </group.AppField>

        <group.AppField name="target">
          {(field) => (
            <field.TextField
              label="Target"
              placeholder="3000"
              inputSuffix={
                <Tooltip>
                  <TooltipTrigger
                    render={
                      <InputGroupButton
                        size="icon-xs"
                        variant="ghost"
                        loading={isFindingPort}
                        onClick={onFillPort}
                        aria-label="Find a free port"
                      >
                        <RefreshIcon />
                      </InputGroupButton>
                    }
                  />
                  <TooltipContent>Find a free port</TooltipContent>
                </Tooltip>
              }
            />
          )}
        </group.AppField>
      </div>
    );
  },
});
