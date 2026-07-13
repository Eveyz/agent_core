import i18n from '../i18n';

/**
 * Format a timestamp as a relative "time ago" string.
 * e.g., "2 minutes ago" / "2分钟前"
 */
export function formatTimeAgo(timestamp: string): string {
  const now = new Date();
  const time = new Date(timestamp);
  const diffMs = now.getTime() - time.getTime();
  const diffSec = Math.floor(diffMs / 1000);
  const diffMin = Math.floor(diffSec / 60);
  const diffHour = Math.floor(diffMin / 60);
  const diffDay = Math.floor(diffHour / 24);
  const diffMonth = Math.floor(diffDay / 30);
  const diffYear = Math.floor(diffDay / 365);

  const plural = (count: number) => (count > 1 ? '_plural' : '');

  if (diffSec < 60) {
    return i18n.t('timeAgo.justNow');
  } else if (diffMin < 60) {
    return i18n.t(`timeAgo.minutes${plural(diffMin)}`, { count: diffMin });
  } else if (diffHour < 24) {
    return i18n.t(`timeAgo.hours${plural(diffHour)}`, { count: diffHour });
  } else if (diffDay < 30) {
    return i18n.t(`timeAgo.days${plural(diffDay)}`, { count: diffDay });
  } else if (diffMonth < 12) {
    return i18n.t(`timeAgo.months${plural(diffMonth)}`, { count: diffMonth });
  } else {
    return i18n.t(`timeAgo.years${plural(diffYear)}`, { count: diffYear });
  }
}
