import { useState, useCallback, createContext, useContext, useEffect, useRef } from 'react';
import XIcon from 'lucide-react/dist/esm/icons/x.mjs';
import AlertCircleIcon from 'lucide-react/dist/esm/icons/alert-circle.mjs';

interface ToastItem {
  id: number;
  message: string;
}

interface ToastContextType {
  showError: (message: string) => void;
}

const ToastContext = createContext<ToastContextType>({ showError: () => {} });

export function useToast() {
  return useContext(ToastContext);
}

let nextId = 0;

export function ToastProvider({ children }: { children: React.ReactNode }) {
  const [toasts, setToasts] = useState<ToastItem[]>([]);
  const timersRef = useRef<Map<number, ReturnType<typeof setTimeout>>>(new Map());

  const removeToast = useCallback((id: number) => {
    setToasts((prev) => prev.filter((t) => t.id !== id));
    const timer = timersRef.current.get(id);
    if (timer) clearTimeout(timer);
    timersRef.current.delete(id);
  }, []);

  const showError = useCallback((message: string) => {
    const id = nextId++;
    setToasts((prev) => [...prev, { id, message }]);
    const timer = setTimeout(() => removeToast(id), 5000);
    timersRef.current.set(id, timer);
  }, [removeToast]);

  useEffect(() => () => {
    timersRef.current.forEach((timer) => clearTimeout(timer));
  }, []);

  return (
    <ToastContext.Provider value={{ showError }}>
      {children}
      <div style={{
        position: 'fixed',
        bottom: '20px',
        right: '20px',
        zIndex: 10000,
        display: 'flex',
        flexDirection: 'column',
        gap: '8px',
        pointerEvents: 'none',
      }}>
        {toasts.map((toast) => (
          <div
            key={toast.id}
            style={{
              background: 'var(--bg-secondary)',
              border: '1px solid var(--danger, #ef4444)',
              borderRadius: '8px',
              padding: '12px 16px',
              display: 'flex',
              alignItems: 'center',
              gap: '8px',
              fontSize: '13px',
              color: 'var(--text-main)',
              boxShadow: '0 4px 12px rgba(0,0,0,0.15)',
              pointerEvents: 'auto',
              maxWidth: '400px',
              animation: 'toastSlideIn 0.3s ease-out',
            }}
          >
            <AlertCircleIcon size={14} color="var(--danger)" style={{ flexShrink: 0 }} />
            <span style={{ flex: 1, wordBreak: 'break-word' }}>{toast.message}</span>
            <button
              onClick={() => removeToast(toast.id)}
              style={{ background: 'none', border: 'none', cursor: 'pointer', color: 'var(--text-muted)', padding: 0, display: 'flex', flexShrink: 0 }}
              aria-label="Dismiss"
            >
              <XIcon size={12} />
            </button>
          </div>
        ))}
      </div>
    </ToastContext.Provider>
  );
}
