import { useState, memo, useMemo } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useStore } from 'react-redux';
import { useAppDispatch } from '../../hooks/useAppDispatch';
import { clarificationAnswered } from '../../features/chat/chatSlice';
import type { RootState } from '../../store';
import type { ClarificationAnswers, ClarificationQuestion } from '../../features/chat/types';

export type ClarificationBlock = {
  type: 'clarification';
  prompt_id: string;
  title?: string;
  questions: ClarificationQuestion[];
  status: 'pending' | 'answered' | 'cancelled';
  answers?: ClarificationAnswers;
};

const ClarificationOverlay = memo(function ClarificationOverlay({
  block,
  isOverlay = false,
}: {
  block: ClarificationBlock;
  isOverlay?: boolean;
}) {
  const dispatch = useAppDispatch();
  const store = useStore<RootState>();
  const [selections, setSelections] = useState<ClarificationAnswers>({});
  const [submitting, setSubmitting] = useState(false);

  const promptId = block.prompt_id ?? '';
  const questions = block.questions ?? [];

  const allAnswered = useMemo(() => {
    return questions.every((q) => (selections[q.id]?.length ?? 0) > 0);
  }, [questions, selections]);

  const toggleOption = (q: ClarificationQuestion, optionId: string) => {
    setSelections((prev) => {
      const current = prev[q.id] ?? [];
      if (q.allow_multiple) {
        const next = current.includes(optionId)
          ? current.filter((id) => id !== optionId)
          : [...current, optionId];
        return { ...prev, [q.id]: next };
      }
      return { ...prev, [q.id]: [optionId] };
    });
  };

  const handleSubmit = async () => {
    if (!allAnswered || submitting) return;
    setSubmitting(true);
    const sessionId = store.getState().project.activeSessionId;
    if (!sessionId) {
      setSubmitting(false);
      return;
    }
    const runId = store.getState().chat.runId[sessionId] ?? null;
    dispatch(clarificationAnswered({ sessionId, promptId, answers: selections }));
    try {
      await invoke('answer_input', {
        promptId,
        runId,
        answer: JSON.stringify({ answers: selections }),
      });
    } catch (e) {
      console.error('Failed to answer clarification', e);
      setSubmitting(false);
    }
  };

  if (block.status === 'answered' || block.status === 'cancelled') {
    const answers = block.answers ?? {};
    return (
      <div className="clarification-block clarification-resolved">
        <div className="clarification-header">
          <span className="clarification-title">{block.title || 'Clarification'}</span>
          <span className="clarification-status-badge status-answered">Answered</span>
        </div>
        <ul className="clarification-summary">
          {questions.map((q) => {
            const selected = answers[q.id] ?? [];
            const labels = selected
              .map((oid) => q.options.find((o) => o.id === oid)?.label ?? oid)
              .join(', ');
            return (
              <li key={q.id}>
                <strong>{q.prompt}</strong>
                <span>{labels || '—'}</span>
              </li>
            );
          })}
        </ul>
      </div>
    );
  }

  const containerClass = isOverlay ? 'clarification-overlay-card' : 'clarification-block';

  return (
    <div className={containerClass}>
      <div className="clarification-header">
        <span className="clarification-title">{block.title || 'Need your input to proceed'}</span>
      </div>
      <div className="clarification-questions">
        {questions.map((q) => (
          <div key={q.id} className="clarification-question">
            <div className="clarification-prompt">
              {q.prompt}
              {q.allow_multiple ? (
                <span className="clarification-multi-hint"> (select all that apply)</span>
              ) : null}
            </div>
            <div className="clarification-options">
              {q.options.map((opt) => {
                const selected = (selections[q.id] ?? []).includes(opt.id);
                return (
                  <button
                    key={opt.id}
                    type="button"
                    className={`clarification-option ${selected ? 'selected' : ''}`}
                    onClick={() => toggleOption(q, opt.id)}
                  >
                    <span
                      className={`clarification-check ${q.allow_multiple ? 'multi' : 'single'} ${selected ? 'on' : ''}`}
                      aria-hidden
                    />
                    {opt.label}
                  </button>
                );
              })}
            </div>
          </div>
        ))}
      </div>
      <div className="clarification-actions">
        <button
          className="btn-allow"
          disabled={!allAnswered || submitting}
          onClick={handleSubmit}
        >
          {submitting ? 'Submitting…' : 'Continue'}
        </button>
      </div>
    </div>
  );
});

export default ClarificationOverlay;
