import { getCurrentWindow } from '@tauri-apps/api/window';
import { Minus, Square, X } from 'lucide-react';
import { cn } from '@/lib/utils';

function LayersMark({ className }: { className?: string }) {
  return (
    <div
      className={cn(
        'flex size-[30px] items-center justify-center rounded-[9px] bg-gradient-to-b from-[#3380f3] to-[#1f5fd6] shadow-[0_3px_8px_rgba(36,99,235,.4)]',
        className,
      )}
    >
      <svg
        fill="none"
        height="17"
        stroke="#fff"
        strokeLinecap="round"
        strokeLinejoin="round"
        strokeWidth="1.9"
        viewBox="0 0 24 24"
        width="17"
      >
        <path d="M12 2 2 7l10 5 10-5L12 2Z" />
        <path d="m2 12 10 5 10-5" />
        <path d="m2 17 10 5 10-5" />
      </svg>
    </div>
  );
}

export type Tab = 'studio' | 'catalog' | 'serving';

export function Titlebar({ status }: { status: React.ReactNode }) {
  const win = getCurrentWindow();
  return (
    <header
      className="relative z-30 flex h-12 flex-none items-center gap-3 border-b border-line bg-[#fbfbfc] pr-1 pl-4"
      data-tauri-drag-region
    >
      <div className="pointer-events-none flex items-center gap-2.5">
        <LayersMark />
        <div className="leading-tight">
          <div className="font-semibold text-[14px] tracking-[-0.01em] text-ink">
            Map Tile Studio
          </div>
          <div className="font-medium text-[10px] tracking-[0.02em] text-faint">by AiGeoLAB</div>
        </div>
      </div>

      <div className="flex-1" data-tauri-drag-region />

      <div className="pointer-events-auto">{status}</div>

      {/* window controls */}
      <div className="ml-1 flex items-center">
        <button
          className="inline-flex h-12 w-[46px] items-center justify-center text-muted transition-colors hover:bg-[#eef0f3]"
          onClick={() => win.minimize()}
          type="button"
        >
          <Minus className="size-[17px]" />
        </button>
        <button
          className="inline-flex h-12 w-[46px] items-center justify-center text-muted transition-colors hover:bg-[#eef0f3]"
          onClick={() => win.toggleMaximize()}
          type="button"
        >
          <Square className="size-[13px]" />
        </button>
        <button
          className="inline-flex h-12 w-[46px] items-center justify-center text-muted transition-colors hover:bg-[#e5484d] hover:text-white"
          onClick={() => win.close()}
          type="button"
        >
          <X className="size-[18px]" />
        </button>
      </div>
    </header>
  );
}
