import assert from 'node:assert/strict';
import test from 'node:test';
import * as grpc from '@grpc/grpc-js';

import { ThewayGrpcClient } from '../src/client.js';
import { CommandServiceService, CommandResult } from '../src/generated/commands.js';
import { EventServiceService, StreamFrame } from '../src/generated/events.js';
import { GraphEngineServiceService } from '../src/generated/graph_engine.js';
import {
  CreateSessionResponse,
  SessionServiceService,
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
