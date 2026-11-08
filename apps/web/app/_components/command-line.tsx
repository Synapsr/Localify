// A command to copy, selected whole on click.
export const CommandLine = ({ command }: { command: string }) => (
  <code className="block overflow-x-auto rounded-lg border border-border/60 bg-muted/60 px-3 py-2 font-mono text-[13px] whitespace-nowrap text-foreground select-all">
    {command}
  </code>
);
