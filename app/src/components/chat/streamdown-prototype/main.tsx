import ReactDOM from 'react-dom/client';
import '@fontsource/geist-sans';
import '@fontsource/geist-mono';
import { renderStreamdownPrototype } from './StreamdownPrototype';

renderStreamdownPrototype(
  ReactDOM.createRoot(document.getElementById('root')!),
);
