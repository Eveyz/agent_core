import React from 'react'
import ReactDOM from 'react-dom/client'
import '@fontsource/geist-sans'
import '@fontsource/geist-mono'
import { Provider } from 'react-redux'
import { store } from './store'
import App from './App'
import { ErrorBoundary } from './components/ErrorBoundary'
import { ToastProvider } from './components/ui/Toast'
import './i18n'


ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <Provider store={store}>
      <ErrorBoundary>
        <ToastProvider>
          <App />
        </ToastProvider>
      </ErrorBoundary>
    </Provider>
  </React.StrictMode>,
);
