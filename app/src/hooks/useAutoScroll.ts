import { useEffect, useRef, useCallback, useState, useLayoutEffect } from 'react';

interface UseAutoScrollOptions {
  /** 当这些依赖变化时，如果处于自动滚动模式，立即贴底（用于新消息、切换会话等） */
  deps: any[];
  /** 是否为处理中状态——为 true 时启动 rAF 循环持续贴底 */
  isProcessing: boolean;
}

export function useAutoScroll<
  T extends HTMLElement,
  U extends HTMLElement = HTMLDivElement,
>(options: UseAutoScrollOptions) {
  const { deps, isProcessing } = options;
  const scrollRef = useRef<T | null>(null);
  const contentRef = useRef<U | null>(null);
  const [isAtBottom, setIsAtBottom] = useState(true);

  // 是否允许自动贴底（用户上滚时变为 false）
  const isAutoScrollEnabled = useRef(true);

  // 暴露给外部强制贴底（切换会话、新消息时调用）
  const scrollToBottom = useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;

    // 同步立刻滚一次（内容已在 DOM 时立即生效）
    el.scrollTop = el.scrollHeight;

    // 再跟 2 帧 rAF 作为兜底（覆盖 React 异步渲染还没完成的边缘情况）
    requestAnimationFrame(() => {
      if (scrollRef.current) {
        scrollRef.current.scrollTop = scrollRef.current.scrollHeight;
      }
    });
    requestAnimationFrame(() => {
      if (scrollRef.current) {
        scrollRef.current.scrollTop = scrollRef.current.scrollHeight;
      }
    });

    isAutoScrollEnabled.current = true;
    setIsAtBottom(true);
  }, []);

  // 1. 依赖变化时同步贴底（新消息、切换会话等非流式场景）
  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    if (isAutoScrollEnabled.current) {
      el.scrollTop = el.scrollHeight;
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, deps);

  // 2. 核心修复：isProcessing 为 true 时，rAF 循环持续贴底
  //    流式输出的每一帧都会触发，不依赖 React 的依赖数组
  useEffect(() => {
    if (!isProcessing) return;

    let id: number;

    const tick = () => {
      const el = scrollRef.current;
      if (el && isAutoScrollEnabled.current) {
        // 只有在确实不在底部时才设置，避免无意义写入
        const maxScroll = el.scrollHeight - el.clientHeight;
        if (el.scrollTop < maxScroll - 1) {
          el.scrollTop = maxScroll;
        }
      }
      id = requestAnimationFrame(tick);
    };

    // 启动循环
    id = requestAnimationFrame(tick);

    return () => cancelAnimationFrame(id);
  }, [isProcessing]);

  // 3. 监听用户手动滚动，决定是否解除自动贴底
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;

    const handleScroll = () => {
      // 20px 阈值，兼容亚像素渲染和微小布局偏移
      const threshold = 20;
      const maxScroll = el.scrollHeight - el.clientHeight;
      const isNearBottom = maxScroll - el.scrollTop <= threshold;

      isAutoScrollEnabled.current = isNearBottom;
      setIsAtBottom(isNearBottom);

      // 用户滑回底部时，立即贴底一次，避免等下一帧 rAF 才跳
      if (isNearBottom && isProcessing) {
        el.scrollTop = maxScroll;
      }
    };

    // passive: true 不阻塞滚动
    el.addEventListener('scroll', handleScroll, { passive: true });

    return () => el.removeEventListener('scroll', handleScroll);
  }, []);

  return { scrollRef, contentRef, scrollToBottom, isAtBottom };
}
