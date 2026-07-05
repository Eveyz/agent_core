import 'react';

declare module 'react' {
  interface CSSProperties {
    WebkitAppRegion?: 'drag' | 'no-drag';
    appRegion?: 'drag' | 'no-drag';
  }
}
