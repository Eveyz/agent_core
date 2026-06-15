import { memo } from 'react';

export const CosmicBackground = memo(function CosmicBackground() {
  return (
    <>
      <div className="cosmic-glow cosmic-glow-1" />
      <div className="cosmic-glow cosmic-glow-2" />
      <div className="cosmic-glow cosmic-glow-3" />
      <div className="cosmic-glow cosmic-glow-4" />
      <div className="star-field" />
    </>
  );
});
