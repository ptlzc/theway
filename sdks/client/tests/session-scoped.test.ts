import assert from 'node:assert/strict';
import test from 'node:test';
import * as grpc from '@grpc/grpc-js';

import { ThewayGrpcClient } from '../src/client.js';
import { CommandServiceService, CommandResult } from '../src/generated/commands.js';
import { EventServiceService, StreamFrame } from '../src/generated/events.js';
import { GraphEngineServiceService } from '../src/generated/graph_engine.js';
import {
  CollapseSessionResponse,
  CreateSessionResponse,
  GetSessionGraphNodeResponse,
  ListSessionGraphNodeMessagesResponse,
  SessionGraphNodeStreamFrame,
  SessionServiceService,
  SessionSnapshot,
  SessionState,
  UpdateSessionMetadataRequest,
} from '../src/generated/session.js';

interface CapturedRequest {
  method: string;
  request: unknown;
}

async function startServer(): Promise<{
  port: number;
  requests: CapturedRequest[];
  close: () => Promise<void>;
}> {
  const server = new grpc.Server();
  const requests: CapturedRequest[] = [];

  server.addService(
    CommandServiceService,
    {
      sendMessage(call: any, callback: any) {
        requests.push({ method: 'SendMessage', request: call.request });
        callback(null, CommandResult.create({ accepted: true }));
      },
      setModel(call: any, callback: any) {
        requests.push({ method: 'SetModel', request: call.request });
        callback(null, CommandResult.create({ accepted: true }));
      },
      setThinking(call: any, callback: any) {
        requests.push({ method: 'SetThinking', request: call.request });
        callback(null, CommandResult.create({ accepted: true }));
      },
      cancel(call: any, callback: any) {
        requests.push({ method: 'Cancel', request: call.request });
        callback(null, CommandResult.create({ accepted: true }));
      },
      approve(call: any, callback: any) {
        requests.push({ method: 'Approve', request: call.request });
        callback(null, CommandResult.create({ accepted: true }));
      },
    } as any,
  );

  server.addService(
    SessionServiceService,
    {
      getState(call: any, callback: any) {
        requests.push({ method: 'GetState', request: call.request });
        callback(null, SessionState.create({ sessionId: call.request.sessionId }));
      },
      getSnapshot(call: any, callback: any) {
        requests.push({ method: 'GetSnapshot', request: call.request });
        callback(
          null,
          SessionSnapshot.create({
            sessionId: call.request.sessionId,
            info: { id: call.request.sessionId, name: 'main', cwd: '/tmp/theway' },
            runtime: { model: { provider: 'provider', model: 'model' } },
            feed: { blocks: [], lines: ['hello'] },
            graphState: { dags: [], subagents: [], nodes: [] },
            lineage: { childSessionIds: [], ancestorSessionIds: [] },
          }),
        );
      },
      getHistory(call: any, callback: any) {
        requests.push({ method: 'GetHistory', request: call.request });
        callback(
          null,
          SessionSnapshot.create({
            sessionId: call.request.sessionId,
            feed: { blocks: [], lines: ['history'] },
          }),
        );
      },
      collapseSession(call: any, callback: any) {
        requests.push({ method: 'CollapseSession', request: call.request });
        callback(
          null,
          CollapseSessionResponse.create({
            sessionId: call.request.sessionId,
            node: {
              id: 'node-collapsed',
              sessionId: 'sess-new',
              type: 2,
              title: 'Archived',
              summary: 'Summary',
              childNodeIds: [],
              messageCount: 7,
            },
            collapsed: {
              nodeId: 'node-collapsed',
              sessionId: call.request.sessionId,
              title: 'Archived',
              summary: 'Summary',
              messageCount: 7,
              collapsedIntoSessionId: 'sess-new',
              collapsedIntoNodeId: 'node-collapsed',
              originalSessionIds: [call.request.sessionId],
            },
          }),
        );
      },
      getSessionGraphNode(call: any, callback: any) {
        requests.push({ method: 'GetSessionGraphNode', request: call.request });
        callback(
          null,
          GetSessionGraphNodeResponse.create({
            node: {
              id: call.request.nodeId,
              sessionId: call.request.sessionId,
              type: 1,
              title: 'Live',
              summary: '',
              childNodeIds: [],
              messageCount: 0,
            },
          }),
        );
      },
      listSessionGraphNodeMessages(call: any, callback: any) {
        requests.push({ method: 'ListSessionGraphNodeMessages', request: call.request });
        callback(
          null,
          ListSessionGraphNodeMessagesResponse.create({
            blocks: [],
          }),
        );
      },
      streamSessionGraphNode(call: any) {
        requests.push({ method: 'StreamSessionGraphNode', request: call.request });
        call.write(
          SessionGraphNodeStreamFrame.create({
            payload: {
              $case: 'node',
              node: {
                id: call.request.nodeId,
                sessionId: call.request.sessionId,
                type: 1,
                title: 'Live',
                summary: '',
                childNodeIds: [],
                messageCount: 0,
              },
            },
          }),
        );
        call.end();
      },
      createSession(call: any, callback: any) {
        requests.push({ method: 'CreateSession', request: call.request });
        callback(
          null,
          CreateSessionResponse.create({
            session: {
              sessionId: call.request.sessionId,
              metadata: call.request.metadata,
            },
          }),
        );
      },
      updateSessionMetadata(call: any, callback: any) {
        requests.push({ method: 'UpdateSessionMetadata', request: call.request });
        callback(null, CommandResult.create({ accepted: true }));
      },
    } as any,
  );

  server.addService(
    GraphEngineServiceService,
    {
      graphCancel(call: any, callback: any) {
        requests.push({ method: 'GraphCancel', request: call.request });
        callback(null, CommandResult.create({ accepted: true }));
      },
    } as any,
  );

  server.addService(
    EventServiceService,
    {
      streamEvents(call: any) {
        requests.push({ method: 'StreamEvents', request: call.request });
        const frame = StreamFrame.create({
          payload: {
            $case: 'event',
            event: {
              sessionId: call.request.sessionId ?? '',
              kind: {
                $case: 'runStatus',
                runStatus: { runId: 'run-1', status: 'running' },
              },
            },
          },
        });
        call.write(frame);
        call.end();
      },
    } as any,
  );

  const port = await new Promise<number>((resolve, reject) => {
    server.bindAsync(
      '127.0.0.1:0',
      grpc.ServerCredentials.createInsecure(),
      (err, boundPort) => {
        if (err) {
          reject(err);
          return;
        }
        resolve(boundPort);
      },
    );
  });
  server.start();

  return {
    port,
    requests,
    close: () =>
      new Promise<void>((resolve, reject) => {
        server.tryShutdown((err) => {
          if (err) {
            reject(err);
          } else {
            resolve();
          }
        });
      }),
  };
}

test('getState(sessionId) sends the explicit session id', async () => {
  const { port, requests, close } = await startServer();
  try {
    const client = new ThewayGrpcClient(`http://127.0.0.1:${port}`);
    const state = await client.getState('sess-1');
    assert.equal(state.sessionId, 'sess-1');
    assert.equal(requests[0]?.method, 'GetState');
    assert.equal((requests[0]?.request as { sessionId: string }).sessionId, 'sess-1');
    client.close();
  } finally {
    await close();
  }
});

test('prompt(sessionId) sends the explicit session id', async () => {
  const { port, requests, close } = await startServer();
  try {
    const client = new ThewayGrpcClient(`http://127.0.0.1:${port}`);
    await client.prompt('hello', [], 'sess-2');
    assert.equal(requests[0]?.method, 'SendMessage');
    assert.equal((requests[0]?.request as { sessionId?: string }).sessionId, 'sess-2');
    client.close();
  } finally {
    await close();
  }
});

test('prompt(sessionId, text) sends the explicit session id', async () => {
  const { port, requests, close } = await startServer();
  try {
    const client = new ThewayGrpcClient(`http://127.0.0.1:${port}`);
    await client.prompt('sess-2b', 'hello');
    assert.equal(requests[0]?.method, 'SendMessage');
    assert.equal((requests[0]?.request as { sessionId?: string }).sessionId, 'sess-2b');
    client.close();
  } finally {
    await close();
  }
});

test('setModel(sessionId) sends the explicit session id', async () => {
  const { port, requests, close } = await startServer();
  try {
    const client = new ThewayGrpcClient(`http://127.0.0.1:${port}`);
    await client.setModel('anthropic', 'claude-sonnet-4-5', 'sess-3');
    assert.equal(requests[0]?.method, 'SetModel');
    const request = requests[0]?.request as { sessionId: string; spec: string };
    assert.equal(request.sessionId, 'sess-3');
    assert.equal(request.spec, 'anthropic:claude-sonnet-4-5');
    client.close();
  } finally {
    await close();
  }
});

test('setModel(sessionId, spec) sends the explicit session id', async () => {
  const { port, requests, close } = await startServer();
  try {
    const client = new ThewayGrpcClient(`http://127.0.0.1:${port}`);
    await client.setModel('sess-3b', 'anthropic:claude-sonnet-4-5');
    assert.equal(requests[0]?.method, 'SetModel');
    const request = requests[0]?.request as { sessionId: string; spec: string };
    assert.equal(request.sessionId, 'sess-3b');
    assert.equal(request.spec, 'anthropic:claude-sonnet-4-5');
    client.close();
  } finally {
    await close();
  }
});

test('setThinking(sessionId) sends the explicit session id', async () => {
  const { port, requests, close } = await startServer();
  try {
    const client = new ThewayGrpcClient(`http://127.0.0.1:${port}`);
    await client.setThinking('high', 'sess-4');
    assert.equal(requests[0]?.method, 'SetThinking');
    const request = requests[0]?.request as { sessionId: string; level: string };
    assert.equal(request.sessionId, 'sess-4');
    assert.equal(request.level, 'high');
    client.close();
  } finally {
    await close();
  }
});

test('setThinking(sessionId, level) sends the explicit session id', async () => {
  const { port, requests, close } = await startServer();
  try {
    const client = new ThewayGrpcClient(`http://127.0.0.1:${port}`);
    await client.setThinking('sess-4b', 'high');
    assert.equal(requests[0]?.method, 'SetThinking');
    const request = requests[0]?.request as { sessionId: string; level: string };
    assert.equal(request.sessionId, 'sess-4b');
    assert.equal(request.level, 'high');
    client.close();
  } finally {
    await close();
  }
});

test('abort(sessionId) sends the explicit session id', async () => {
  const { port, requests, close } = await startServer();
  try {
    const client = new ThewayGrpcClient(`http://127.0.0.1:${port}`);
    await client.abort('sess-5');
    assert.equal(requests[0]?.method, 'Cancel');
    assert.equal((requests[0]?.request as { sessionId: string }).sessionId, 'sess-5');
    client.close();
  } finally {
    await close();
  }
});

test('resolveControlPlane(sessionId) sends the explicit session id', async () => {
  const { port, requests, close } = await startServer();
  try {
    const client = new ThewayGrpcClient(`http://127.0.0.1:${port}`);
    await client.resolveControlPlane(true, 'sess-6');
    assert.equal(requests[0]?.method, 'Approve');
    const request = requests[0]?.request as { sessionId: string; approve: boolean };
    assert.equal(request.sessionId, 'sess-6');
    assert.equal(request.approve, true);
    client.close();
  } finally {
    await close();
  }
});

test('resolveControlPlane(sessionId, approve) sends the explicit session id', async () => {
  const { port, requests, close } = await startServer();
  try {
    const client = new ThewayGrpcClient(`http://127.0.0.1:${port}`);
    await client.resolveControlPlane('sess-6b', true);
    assert.equal(requests[0]?.method, 'Approve');
    const request = requests[0]?.request as { sessionId: string; approve: boolean };
    assert.equal(request.sessionId, 'sess-6b');
    assert.equal(request.approve, true);
    client.close();
  } finally {
    await close();
  }
});

test('graphCancel(sessionId, runId) sends the explicit session id', async () => {
  const { port, requests, close } = await startServer();
  try {
    const client = new ThewayGrpcClient(`http://127.0.0.1:${port}`);
    await client.graphCancel('sess-7', 'run-123');
    assert.equal(requests[0]?.method, 'GraphCancel');
    const request = requests[0]?.request as { sessionId: string; runId: string };
    assert.equal(request.sessionId, 'sess-7');
    assert.equal(request.runId, 'run-123');
    client.close();
  } finally {
    await close();
  }
});

test('createSession(sessionId, metadata) sends the explicit session id and metadata', async () => {
  const { port, requests, close } = await startServer();
  try {
    const client = new ThewayGrpcClient(`http://127.0.0.1:${port}`);
    const created = await client.createSession('My Session', 'sess-8', { tenant: 'acme', region: 'us' });
    assert.equal(created.session?.sessionId, 'sess-8');
    assert.deepEqual(created.session?.metadata, { tenant: 'acme', region: 'us' });
    assert.equal(requests[0]?.method, 'CreateSession');
    const request = requests[0]?.request as { sessionId?: string; metadata: Record<string, string> };
    assert.equal(request.sessionId, 'sess-8');
    assert.deepEqual(request.metadata, { tenant: 'acme', region: 'us' });
    client.close();
  } finally {
    await close();
  }
});

test('createSession(sessionId, metadata) sends the explicit session id and metadata', async () => {
  const { port, requests, close } = await startServer();
  try {
    const client = new ThewayGrpcClient(`http://127.0.0.1:${port}`);
    const created = await client.createSession('sess-8b', { tenant: 'acme' });
    assert.equal(created.session?.sessionId, 'sess-8b');
    assert.deepEqual(created.session?.metadata, { tenant: 'acme' });
    assert.equal(requests[0]?.method, 'CreateSession');
    const request = requests[0]?.request as { sessionId?: string; metadata: Record<string, string> };
    assert.equal(request.sessionId, 'sess-8b');
    assert.deepEqual(request.metadata, { tenant: 'acme' });
    client.close();
  } finally {
    await close();
  }
});

test('updateSessionMetadata(sessionId, metadata) sends the explicit session id and metadata', async () => {
  const { port, requests, close } = await startServer();
  try {
    const client = new ThewayGrpcClient(`http://127.0.0.1:${port}`);
    await client.updateSessionMetadata('sess-9', { owner: 'platform' });
    assert.equal(requests[0]?.method, 'UpdateSessionMetadata');
    const request = requests[0]?.request as UpdateSessionMetadataRequest;
    assert.equal(request.sessionId, 'sess-9');
    assert.deepEqual(request.metadata, { owner: 'platform' });
    client.close();
  } finally {
    await close();
  }
});

test('openEventStream(sessionId) sends the explicit session id', async () => {
  const { port, requests, close } = await startServer();
  try {
    const client = new ThewayGrpcClient(`http://127.0.0.1:${port}`);
    const seen: string[] = [];
    await client.openEventStream('sess-10', (frame) => {
      if (frame.payload?.$case === 'event') {
        seen.push(frame.payload.event.sessionId);
      }
    });
    assert.deepEqual(seen, ['sess-10']);
    assert.equal(requests[0]?.method, 'StreamEvents');
    assert.equal((requests[0]?.request as { sessionId?: string }).sessionId, 'sess-10');
    client.close();
  } finally {
    await close();
  }
});

test('getSnapshot(sessionId) returns the nested session snapshot', async () => {
  const { port, requests, close } = await startServer();
  try {
    const client = new ThewayGrpcClient(`http://127.0.0.1:${port}`);
    const snapshot = await client.getSnapshot('sess-snap');
    assert.equal(snapshot.sessionId, 'sess-snap');
    assert.equal(snapshot.info?.id, 'sess-snap');
    assert.equal(snapshot.runtime?.model?.provider, 'provider');
    assert.equal(snapshot.feed?.lines[0], 'hello');
    assert.equal(requests[0]?.method, 'GetSnapshot');
    assert.equal((requests[0]?.request as { sessionId: string }).sessionId, 'sess-snap');
    client.close();
  } finally {
    await close();
  }
});

test('getHistory(sessionId) returns the snapshot-shaped transcript', async () => {
  const { port, requests, close } = await startServer();
  try {
    const client = new ThewayGrpcClient(`http://127.0.0.1:${port}`);
    const snapshot = await client.getHistory('sess-hist');
    assert.equal(snapshot.sessionId, 'sess-hist');
    assert.equal(snapshot.feed?.lines[0], 'history');
    assert.equal(requests[0]?.method, 'GetHistory');
    assert.equal((requests[0]?.request as { sessionId: string }).sessionId, 'sess-hist');
    client.close();
  } finally {
    await close();
  }
});

test('collapseSession(request) sends the collapse request and returns the response', async () => {
  const { port, requests, close } = await startServer();
  try {
    const client = new ThewayGrpcClient(`http://127.0.0.1:${port}`);
    const response = await client.collapseSession({
      sessionId: 'sess-old',
      intoSessionId: 'sess-new',
      title: 'Archived',
      summary: 'Summary',
    });
    assert.equal(response.sessionId, 'sess-old');
    assert.equal(response.node?.id, 'node-collapsed');
    assert.equal(response.collapsed?.collapsedIntoSessionId, 'sess-new');
    assert.equal(requests[0]?.method, 'CollapseSession');
    const request = requests[0]?.request as {
      sessionId: string;
      intoSessionId?: string;
      title?: string;
      summary?: string;
    };
    assert.equal(request.sessionId, 'sess-old');
    assert.equal(request.intoSessionId, 'sess-new');
    assert.equal(request.title, 'Archived');
    client.close();
  } finally {
    await close();
  }
});

test('getSessionGraphNode(sessionId, nodeId) returns the node', async () => {
  const { port, requests, close } = await startServer();
  try {
    const client = new ThewayGrpcClient(`http://127.0.0.1:${port}`);
    const node = await client.getSessionGraphNode('sess-new', 'node-1');
    assert.equal(node.id, 'node-1');
    assert.equal(node.sessionId, 'sess-new');
    assert.equal(requests[0]?.method, 'GetSessionGraphNode');
    const request = requests[0]?.request as { sessionId: string; nodeId: string };
    assert.equal(request.sessionId, 'sess-new');
    assert.equal(request.nodeId, 'node-1');
    client.close();
  } finally {
    await close();
  }
});

test('listSessionGraphNodeMessages(sessionId, nodeId, offset, limit) sends pagination', async () => {
  const { port, requests, close } = await startServer();
  try {
    const client = new ThewayGrpcClient(`http://127.0.0.1:${port}`);
    const response = await client.listSessionGraphNodeMessages('sess-new', 'node-1', 10, 25);
    assert.deepEqual(response.blocks, []);
    assert.equal(requests[0]?.method, 'ListSessionGraphNodeMessages');
    const request = requests[0]?.request as {
      sessionId: string;
      nodeId: string;
      offset: number;
      limit: number;
    };
    assert.equal(request.sessionId, 'sess-new');
    assert.equal(request.nodeId, 'node-1');
    assert.equal(request.offset, 10);
    assert.equal(request.limit, 25);
    client.close();
  } finally {
    await close();
  }
});

test('streamSessionGraphNode(sessionId, nodeId) yields session graph frames', async () => {
  const { port, requests, close } = await startServer();
  try {
    const client = new ThewayGrpcClient(`http://127.0.0.1:${port}`);
    const seen: Array<{ id?: string }> = [];
    await new Promise<void>((resolve, reject) => {
      const stream = client.streamSessionGraphNode('sess-new', 'node-1');
      stream.on('data', (frame: SessionGraphNodeStreamFrame) => {
        if (frame.payload?.$case === 'node') {
          seen.push({ id: frame.payload.node.id });
        }
      });
      stream.on('end', () => resolve());
      stream.on('error', reject);
    });
    assert.deepEqual(seen, [{ id: 'node-1' }]);
    assert.equal(requests[0]?.method, 'StreamSessionGraphNode');
    const request = requests[0]?.request as { sessionId: string; nodeId: string };
    assert.equal(request.sessionId, 'sess-new');
    assert.equal(request.nodeId, 'node-1');
    client.close();
  } finally {
    await close();
  }
});
