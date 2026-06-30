import sqlite3, json, sys, io

sys.stdout = io.TextIOWrapper(sys.stdout.buffer, encoding='utf-8')

DB = r'C:\Users\DY2024\.local\share\mimocode\mimocode.db'
conn = sqlite3.connect(DB)
c = conn.cursor()

# Search across ALL sessions for this project for user decisions/rules
c.execute('''SELECT p.id, p.data, p.time_created, m.data as msg_data
             FROM part p
             JOIN message m ON p.message_id = m.id
             WHERE p.session_id IN (
               SELECT id FROM session WHERE project_id='d739a3ee-8c4f-4868-868f-0a6219026713'
             )
             ORDER BY p.time_created DESC LIMIT 500''')

keywords = ['always', 'never', 'remember', 'rule', 'decision', '决定', '必须', '不要', '可以', '记住', 
            '不需要', '不要用', '需要', '必须', '删除', '移除', '改名', '重命名',
            '不需要考虑兼容性', '只关注', '不管', '不考虑', '不改']

user_decisions = []
for row in c.fetchall():
    part_data = json.loads(row[1])
    msg_data = json.loads(row[3])
    role = msg_data.get('role', '')
    if role != 'user':
        continue
    if part_data.get('type') != 'text':
        continue
    text = part_data.get('text', '')
    text_lower = text.lower()
    for kw in keywords:
        if kw.lower() in text_lower:
            user_decisions.append((row[0], text[:400], row[2]))
            break

print("=== USER DECISIONS/RULES FOUND IN TRAJECTORY ===")
for pid, text, ts in user_decisions[:30]:
    print("[%s] %s" % (pid, text[:300]))
    print("---")

# Also search for assistant text mentioning "decision" or "decided"
print("\n=== ASSISTANT DECISIONS MENTIONS ===")
c.execute('''SELECT p.id, p.data, p.time_created
             FROM part p
             WHERE p.session_id IN (
               SELECT id FROM session WHERE project_id='d739a3ee-8c4f-4868-868f-0a6219026713'
             )
             AND p.session_id != 'ses_0e81e2ce1ffe8n77MDci9niIXq'
             ORDER BY p.time_created DESC LIMIT 300''')

for row in c.fetchall():
    part_data = json.loads(row[1])
    if part_data.get('type') != 'text':
        continue
    text = part_data.get('text', '')
    text_lower = text.lower()
    if any(kw in text_lower for kw in ['decision', 'decided', 'decided to', 'the user said', 'user requested', 'user wants']):
        print("[%s] %s" % (row[0], text[:300]))
        print("---")

conn.close()
