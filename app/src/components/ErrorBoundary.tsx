import { Component, ErrorInfo, ReactNode } from 'react';

interface Props {
  children: ReactNode;
}

interface State {
  hasError: boolean;
  error: Error | null;
}

export class ErrorBoundary extends Component<Props, State> {
  constructor(props: Props) {
    super(props);
    this.state = { hasError: false, error: null };
  }

  static getDerivedStateFromError(error: Error): State {
    return { hasError: true, error };
  }

  componentDidCatch(error: Error, errorInfo: ErrorInfo): void {
    console.error('[ErrorBoundary]', error, errorInfo);
  }

  handleReset = (): void => {
    this.setState({ hasError: false, error: null });
  };

  render(): ReactNode {
    if (this.state.hasError) {
      return (
        <div style={{ display: 'flex', flexDirection: 'column', alignItems: 'center', justifyContent: 'center', height: '100vh', gap: '16px', color: 'var(--text-secondary)' }}>
          <h2 style={{ fontSize: '18px' }}>Something went wrong</h2>
          <pre style={{ maxWidth: '600px', overflow: 'auto', fontSize: '12px', color: 'var(--danger)' }}>
            {this.state.error?.message}
          </pre>
          <button
            onClick={this.handleReset}
            style={{ padding: '8px 16px', cursor: 'pointer', border: '1px solid var(--border-strong)', borderRadius: 'var(--radius-sm)', background: 'var(--bg-surface)', color: 'var(--text-secondary)' }}
          >
            Try Again
          </button>
        </div>
      );
    }

    return this.props.children;
  }
}
