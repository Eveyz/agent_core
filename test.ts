const blocks = [
  { type: 'tool', name: 'invoke_subagent' },
  { type: 'approval', status: 'approved' }
];
const isSubagentTool = (b: any) => b.name === 'invoke_subagent';
const hasRegularTools = blocks.some(b => 
  (b.type === 'tool' && !isSubagentTool(b)) || 
  (b.type === 'approval' && b.status === 'pending')
);
console.log(hasRegularTools);
