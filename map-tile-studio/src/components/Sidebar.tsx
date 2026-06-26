import type { ReactNode } from 'react';
import { Grid3x3, Layers, Server } from 'lucide-react';
import { cn } from '@/lib/utils';
import type { Tab } from '@/components/Titlebar';

const ITEMS: { id: Tab; label: string; icon: ReactNode }[] = [
  { id: 'studio', label: 'Studio', icon: <Layers className="size-[19px]" /> },
  { id: 'catalog', label: 'Tiles Catalog', icon: <Grid3x3 className="size-[19px]" /> },
  { id: 'serving', label: 'Serving', icon: <Server className="size-[19px]" /> },
];

/** Thin icon rail. Each item shows its name in a tooltip on hover. */
export function Sidebar({ tab, onTab }: { tab: Tab; onTab: (t: Tab) => void }) {
  return (
    <nav className="z-20 flex w-[60px] flex-none flex-col items-center gap-1.5 border-r border-line bg-[#fbfbfc] py-3.5">
      {ITEMS.map((it) => {
        const active = tab === it.id;
        return (
          <div className="group/nav relative flex justify-center" key={it.id}>
            <button
              aria-label={it.label}
              className={cn(
                'relative flex size-[42px] items-center justify-center rounded-[12px] transition-colors',
                active
                  ? 'bg-brand-tint text-brand'
                  : 'text-[#9aa3af] hover:bg-[#f1f2f5] hover:text-ink-soft',
              )}
              onClick={() => onTab(it.id)}
              type="button"
            >
              {active && (
                <span className="-left-[9px] -translate-y-1/2 absolute top-1/2 h-5 w-[3px] rounded-r-full bg-brand" />
              )}
              {it.icon}
            </button>
            {/* hover tooltip */}
            <span className="-translate-y-1/2 pointer-events-none absolute top-1/2 left-[calc(100%+10px)] z-50 whitespace-nowrap rounded-lg bg-ink px-2.5 py-1.5 font-medium text-[12px] text-white opacity-0 shadow-[0_6px_16px_rgba(16,24,40,.2)] transition-opacity duration-150 group-hover/nav:opacity-100">
              {it.label}
              <span className="-translate-y-1/2 absolute top-1/2 right-full size-0 border-y-[5px] border-r-[5px] border-y-transparent border-r-ink" />
            </span>
          </div>
        );
      })}
    </nav>
  );
}
