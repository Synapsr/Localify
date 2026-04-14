import { Outlet } from "@tanstack/react-router";
import { TopBar } from "./top-bar";

export function AppLayout() {
  return (
    <div className="flex h-full flex-col">
      <TopBar />
      <main className="flex-1 overflow-y-auto">
        <div className="mx-auto w-full max-w-4xl px-8 py-10">
          <Outlet />
        </div>
      </main>
    </div>
  );
}
