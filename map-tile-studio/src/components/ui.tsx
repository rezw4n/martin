import type { ButtonHTMLAttributes, InputHTMLAttributes, ReactNode } from 'react';
import { Loader2 } from 'lucide-react';
import { cn } from '@/lib/utils';

/* ── Button ─────────────────────────────────────────────────────────────── */
type ButtonProps = ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: 'primary' | 'secondary' | 'ghost' | 'subtle';
  size?: 'sm' | 'md' | 'lg';
  busy?: boolean;
};

export function Button({
  variant = 'secondary',
  size = 'md',
  busy,
  className,
  children,
  disabled,
  ...rest
}: ButtonProps) {
  const variants = {
    primary:
      'bg-brand text-white shadow-[0_1px_2px_rgba(36,99,235,.4),0_8px_18px_-8px_rgba(36,99,235,.55)] hover:bg-brand-strong active:translate-y-px disabled:opacity-50 disabled:shadow-none',
    secondary:
      'border border-[#dfe3e9] bg-white text-ink-soft hover:bg-[#f6f7f9] active:bg-[#eef0f3] disabled:opacity-50',
    ghost: 'text-ink-soft hover:bg-[#f1f2f5] disabled:opacity-50',
    subtle: 'bg-brand-tint text-brand hover:bg-[#dde9fe] disabled:opacity-50',
  };
  const sizes = {
    sm: 'h-8 px-3 text-[12.5px] gap-1.5 rounded-lg',
    md: 'h-10 px-4 text-[13px] gap-2 rounded-[10px]',
    lg: 'h-11 px-5 text-[13.5px] gap-2 rounded-[11px]',
  };
  return (
    <button
      className={cn(
        'inline-flex select-none items-center justify-center font-semibold transition-all duration-150 outline-none focus-visible:ring-2 focus-visible:ring-brand/40 disabled:cursor-not-allowed',
        variants[variant],
        sizes[size],
        className,
      )}
      disabled={disabled || busy}
      type="button"
      {...rest}
    >
      {busy && <Loader2 className="size-4 animate-spin" />}
      {children}
    </button>
  );
}

/* ── Icon button ────────────────────────────────────────────────────────── */
export function IconButton({
  className,
  children,
  ...rest
}: ButtonHTMLAttributes<HTMLButtonElement>) {
  return (
    <button
      className={cn(
        'inline-flex size-9 items-center justify-center rounded-lg text-faint transition-colors hover:bg-[#f1f2f5] hover:text-ink-soft outline-none',
        className,
      )}
      type="button"
      {...rest}
    >
      {children}
    </button>
  );
}

/* ── Field label wrapper ────────────────────────────────────────────────── */
export function Field({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: ReactNode;
  children: ReactNode;
}) {
  return (
    <label className="block">
      <div className="mb-[7px] flex items-center justify-between">
        <span className="font-semibold text-[10.5px] uppercase tracking-[0.04em] text-faint">
          {label}
        </span>
        {hint && <span className="text-[11px] text-faint">{hint}</span>}
      </div>
      {children}
    </label>
  );
}

/* ── Text / number input ────────────────────────────────────────────────── */
export function TextInput({
  className,
  mono,
  ...rest
}: InputHTMLAttributes<HTMLInputElement> & { mono?: boolean }) {
  return (
    <input
      className={cn(
        'h-10 w-full rounded-[9px] border border-[#dfe3e9] bg-white px-3 text-[13px] text-ink outline-none transition-colors placeholder:text-faint focus:border-brand focus:ring-2 focus:ring-brand/15',
        mono && 'font-mono',
        className,
      )}
      {...rest}
    />
  );
}

/* ── Segmented control ──────────────────────────────────────────────────── */
export function Segmented<T extends string>({
  options,
  value,
  onChange,
  className,
}: {
  options: { value: T; label: ReactNode }[];
  value: T;
  onChange: (v: T) => void;
  className?: string;
}) {
  return (
    <div
      className={cn(
        'flex gap-[3px] rounded-[10px] border border-[#e9ebef] bg-[#f3f4f6] p-[3px]',
        className,
      )}
    >
      {options.map((o) => {
        const active = o.value === value;
        return (
          <button
            className={cn(
              'flex h-9 flex-1 items-center justify-center gap-1.5 rounded-[7px] text-[12.5px] font-medium transition-all',
              active
                ? 'bg-white font-semibold text-brand shadow-[0_1px_2px_rgba(16,24,40,.1)]'
                : 'text-muted hover:text-ink-soft',
            )}
            key={o.value}
            onClick={() => onChange(o.value)}
            type="button"
          >
            {o.label}
          </button>
        );
      })}
    </div>
  );
}

/* ── Toggle (switch) ────────────────────────────────────────────────────── */
export function Toggle({
  checked,
  onChange,
  title,
  description,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  title: ReactNode;
  description?: ReactNode;
}) {
  return (
    <button
      className="flex w-full items-center gap-3 rounded-[11px] border border-[#e9ebef] bg-[#fafbfd] p-[13px] text-left transition-colors hover:border-[#dbe1ea]"
      onClick={() => onChange(!checked)}
      type="button"
    >
      <span
        className={cn(
          'relative h-[22px] w-[38px] flex-none rounded-full transition-colors duration-200',
          checked ? 'bg-brand' : 'bg-[#cfd6e0]',
        )}
      >
        <span
          className={cn(
            'absolute top-[2px] size-[18px] rounded-full bg-white shadow-[0_1px_2px_rgba(0,0,0,.2)] transition-all duration-200',
            checked ? 'left-[18px]' : 'left-[2px]',
          )}
        />
      </span>
      <span className="min-w-0">
        <span className="block font-semibold text-[13px] text-ink">{title}</span>
        {description && (
          <span className="mt-px block text-[11.5px] leading-snug text-faint">{description}</span>
        )}
      </span>
    </button>
  );
}

/* ── Badge ──────────────────────────────────────────────────────────────── */
export function Badge({
  children,
  tone = 'neutral',
  className,
}: {
  children: ReactNode;
  tone?: 'neutral' | 'brand' | 'ok' | 'warn' | 'danger';
  className?: string;
}) {
  const tones = {
    neutral: 'bg-[#f1f3f5] text-muted',
    brand: 'bg-brand-tint text-brand',
    ok: 'bg-ok-tint text-[#067647]',
    warn: 'bg-warn-tint text-[#b54708]',
    danger: 'bg-danger-tint text-[#b42318]',
  };
  return (
    <span
      className={cn(
        'inline-flex items-center gap-1 rounded-full px-2 py-[3px] font-semibold text-[11px]',
        tones[tone],
        className,
      )}
    >
      {children}
    </span>
  );
}

/* ── Stat ───────────────────────────────────────────────────────────────── */
export function Stat({ label, value }: { label: string; value: ReactNode }) {
  return (
    <div className="rounded-[10px] border border-[#eef0f3] bg-[#fafbfc] px-3 py-2.5">
      <div className="font-semibold text-[10px] uppercase tracking-[0.04em] text-faint">
        {label}
      </div>
      <div className="mt-1 font-semibold text-[14px] text-ink tabular-nums">{value}</div>
    </div>
  );
}

/* ── Indeterminate progress bar ─────────────────────────────────────────── */
export function ProgressBar({ percent }: { percent: number | null }) {
  return (
    <div className="h-1 overflow-hidden rounded-full bg-[#eef1f5]">
      {percent == null ? (
        <span
          className="block h-full w-[30%] rounded-full bg-gradient-to-r from-[#7aa8f5] to-brand"
          style={{ animation: 'mts-indet 1.6s ease-in-out infinite' }}
        />
      ) : (
        <span
          className="block h-full rounded-full bg-gradient-to-r from-[#7aa8f5] to-brand transition-[width] duration-300"
          style={{ width: `${Math.max(4, Math.min(100, percent))}%` }}
        />
      )}
    </div>
  );
}
