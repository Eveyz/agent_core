import React from 'react'
import ReactDOM from 'react-dom/client'
import '@fontsource/geist-sans'
import '@fontsource/geist-mono'
import { Provider } from 'react-redux'
import { store } from './store'
import App from './App'
import { ErrorBoundary } from './components/ErrorBoundary'

async function init() {
  if (import.meta.env.DEV) {
    const whyDidYouRender = await import('@welldone-software/why-did-you-render');
    whyDidYouRender.default(React, {
      trackAllPureComponents: true,
      notifier: (updateInfo: any) => {
        const report = {
          component: updateInfo.Component.displayName || updateInfo.Component.name,
          reason: updateInfo.reason,
          time: new Date().toISOString()
        };
        console.warn('WDYR_REPORT:', JSON.stringify(report));
      }
    });
  }

  ReactDOM.createRoot(document.getElementById('root')!).render(
    <React.StrictMode>
      <Provider store={store}>
        <ErrorBoundary>
          <App />
        </ErrorBoundary>
      </Provider>
    </React.StrictMode>,
  );
}

init();
