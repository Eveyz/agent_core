import { memo, useState, useCallback, useEffect, useRef } from 'react';

export interface ConfirmDialogState {
  title: string;
  message: string;
  confirmLabel?: string;
  cancelLabel?: string;
  danger?: boolean;
  resolve: (confirmed: boolean) => void;
}

export interface PromptDialogState {
  title: string;
  message: string;
  defaultValue?: string;
  confirmLabel?: string;
  cancelLabel?: string;
  danger?: boolean;
  resolve: (value: string | null) => void;
}

export type DialogState = ConfirmDialogState | PromptDialogState | null;

/**
 * Dialog manager component (P2-6).
 *
 * Replaces native confirm()/prompt()/alert() calls that block the UI thread
 * and behave inconsistently in Tauri WebView. The parent component holds the
 * dialog state and passes it down.
 */
export const DialogManager = memo(function DialogManager({
  state,
  onClose,
}: {
  state: DialogState;
  onClose: () => void;
}) {
  const inputRef = useRef<HTMLInputElement>(null);
  const [inputValue, setInputValue] = useState('');

  useEffect(() => {
    if (state && 'defaultValue' in state) {
      setInputValue(state.defaultValue ?? '');
      setTimeout(() => inputRef.current?.focus(), 50);
    }
  }, [state]);

  const handleConfirm = useCallback(() => {
    if (state) {
      if ('resolve' in state) {
        if ('defaultValue' in state) {
          (state as PromptDialogState).resolve(inputValue);
        } else {
          (state as ConfirmDialogState).resolve(true);
        }
      }
    }
    onClose();
  }, [state, inputValue, onClose]);

  const handleCancel = useCallback(() => {
    if (state) {
      if ('defaultValue' in state) {
        (state as PromptDialogState).resolve(null);
      } else {
        (state as ConfirmDialogState).resolve(false);
      }
    }
    onClose();
  }, [state, onClose]);

  useEffect(() => {
    if (!state) return;
    const handler = (e: KeyboardEvent) => {
      if (e.key === 'Escape') handleCancel();
      if (e.key === 'Enter' && 'defaultValue' in state) handleConfirm();
    };
    document.addEventListener('keydown', handler);
    return () => document.removeEventListener('keydown', handler);
  }, [state, handleCancel, handleConfirm]);

  if (!state) return null;

  const isPrompt = 'defaultValue' in state;
  const confirmLabel = state.confirmLabel ?? 'Confirm';
  const cancelLabel = state.cancelLabel ?? 'Cancel';

  return (
    <div className="dialog-overlay" onClick={handleCancel}>
      <div className="dialog-content" onClick={(e) => e.stopPropagation()}>
        <h3 className="dialog-title">{state.title}</h3>
        <p className="dialog-message">{state.message}</p>
        {isPrompt && (
          <input
            ref={inputRef}
            className="dialog-input"
            value={inputValue}
            onChange={(e) => setInputValue(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') handleConfirm();
              if (e.key === 'Escape') handleCancel();
            }}
          />
        )}
        <div className="dialog-actions">
          <button className="btn-cancel" onClick={handleCancel}>
            {cancelLabel}
          </button>
          <button
            className={`btn-confirm ${state.danger ? 'btn-deny' : 'btn-allow'}`}
            onClick={handleConfirm}
          >
            {confirmLabel}
          </button>
        </div>
      </div>
    </div>
  );
});

/**
 * Hook for async confirm dialog (P2-6).
 * Returns [confirm, dialogElement] — render dialogElement in your component.
 */
export function useConfirmDialog() {
  const [dialogState, setDialogState] = useState<DialogState>(null);

  const confirm = useCallback((opts: {
    title: string;
    message: string;
    confirmLabel?: string;
    cancelLabel?: string;
    danger?: boolean;
  }): Promise<boolean> => {
    return new Promise<boolean>((resolve) => {
      setDialogState({ ...opts, resolve });
    });
  }, []);

  const prompt = useCallback((opts: {
    title: string;
    message: string;
    defaultValue?: string;
    confirmLabel?: string;
    cancelLabel?: string;
  }): Promise<string | null> => {
    return new Promise<string | null>((resolve) => {
      setDialogState({ ...opts, resolve });
    });
  }, []);

  const dialogElement = <DialogManager state={dialogState} onClose={() => setDialogState(null)} />;

  return { confirm, prompt, dialogElement };
}
