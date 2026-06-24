import { useState, useEffect, memo } from 'react';
import { formatTime } from '../../utils/format';

const ProcessingTimer = memo(function ProcessingTimer({
  startTime,
  endTime,
}: {
  startTime?: number;
  endTime?: number;
}) {
  const [now, setNow] = useState(Date.now());

  useEffect(() => {
    if (!startTime || endTime) return;
    // PERF-7: 1s granularity is sufficient for "Processed 3s" display.
    // Previously 250ms caused 4 unnecessary re-renders/sec during streaming.
    const interval = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(interval);
  }, [startTime, endTime]);

  if (!startTime) return null;
  if (endTime) return <span>Processed {formatTime(endTime - startTime)}</span>;
  const diff = now - startTime;
  return <span>Processed {formatTime(diff)}</span>;
});

export default ProcessingTimer;
