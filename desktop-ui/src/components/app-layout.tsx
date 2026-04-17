import { PageTransition } from "./shared/page-transition";
import { SharedNavigation } from "./shared/shared-navigation";

export function AppLayout() {
  return (
    <div data-tauri-drag-region className="flex h-full flex-col bg-neutral-100 p-1.5 pt-8 ">
      <div className="flex h-full flex-col bg-white rounded-lg">
        <SharedNavigation />
        <main className="flex-1 overflow-hidden  rounded-lg">
          <PageTransition />
        </main>
      </div>
    </div>
  );
}
