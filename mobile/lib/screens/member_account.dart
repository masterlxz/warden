import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../protocol/messages.dart';
import '../services/server_connection.dart';

// P84 fatia 4 and 5 on the phone: what a member sees about their own data. Everything here mirrors
// the web's `ChangePasswordView`, `RecoveryCodeView` and `RecoveryNoticeView`, and talks to the same
// hub messages.

const minPasswordLength = 8;

/// Shows a recovery code once. There's no way to dismiss it before "I wrote it down" is ticked: the
/// code is the only way back to the data after a forgotten password, and the hub can't show it again.
Future<void> showRecoveryCode(BuildContext context, String code, {bool replacing = false}) {
  return showDialog<void>(
    context: context,
    barrierDismissible: false,
    builder: (_) => PopScope(canPop: false, child: RecoveryCodeDialog(code: code, replacing: replacing)),
  );
}

class RecoveryCodeDialog extends StatefulWidget {
  const RecoveryCodeDialog({super.key, required this.code, this.replacing = false});

  final String code;

  /// A code the member asked for (or a new policy made): the old one stopped working.
  final bool replacing;

  @override
  State<RecoveryCodeDialog> createState() => _RecoveryCodeDialogState();
}

class _RecoveryCodeDialogState extends State<RecoveryCodeDialog> {
  bool _saved = false;

  @override
  Widget build(BuildContext context) {
    return AlertDialog(
      title: Text(widget.replacing ? 'Your new recovery code' : 'Your data is now encrypted'),
      content: SingleChildScrollView(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(widget.replacing
                ? 'The old code stopped working. Keep this one: it is the only way to recover everything if you forget your password, or if whoever runs the hub resets it.'
                : 'Keep this code: it is the only way to recover everything if you forget your password, or if whoever runs the hub resets it.'),
            const SizedBox(height: 12),
            SelectableText(widget.code, key: const Key('recovery-code'), style: const TextStyle(fontFamily: 'monospace', fontSize: 16, fontWeight: FontWeight.w600)),
            TextButton.icon(
              onPressed: () => Clipboard.setData(ClipboardData(text: widget.code)),
              icon: const Icon(Icons.copy, size: 18),
              label: const Text('Copy'),
            ),
            const Text('It is shown only this once, and nobody on the hub can see it afterwards — not even whoever runs it. Write it down somewhere safe, away from this phone.'),
            CheckboxListTile(
              key: const Key('recovery-code-saved'),
              contentPadding: EdgeInsets.zero,
              controlAffinity: ListTileControlAffinity.leading,
              value: _saved,
              onChanged: (value) => setState(() => _saved = value ?? false),
              title: const Text('I saved the code somewhere safe'),
            ),
          ],
        ),
      ),
      actions: [
        FilledButton(onPressed: _saved ? () => Navigator.of(context).pop() : null, child: const Text('Continue')),
      ],
    );
  }
}

/// What a password change gave back: the recovery code, when it turned encryption on.
class PasswordChange {
  const PasswordChange({this.recoveryCode});

  final String? recoveryCode;
}

/// Swaps the provisional password for the member's own (first sign-in, [required]) or changes it later.
/// [needsRecovery]: the owner reset the password of someone whose data is encrypted, so the recovery
/// code is asked for too. Pops a [PasswordChange] once the hub took it, null if they cancel.
class ChangePasswordDialog extends StatefulWidget {
  const ChangePasswordDialog({super.key, required this.connection, required this.name, this.required = true, this.needsRecovery = false});

  final ServerConnection connection;
  final String name;
  final bool required;
  final bool needsRecovery;

  @override
  State<ChangePasswordDialog> createState() => _ChangePasswordDialogState();
}

class _ChangePasswordDialogState extends State<ChangePasswordDialog> {
  final _current = TextEditingController();
  final _next = TextEditingController();
  final _again = TextEditingController();
  final _code = TextEditingController();
  String? _error;
  bool _busy = false;

  Future<void> _submit() async {
    if (_next.text.length < minPasswordLength) {
      setState(() => _error = 'The new password needs at least $minPasswordLength characters.');
      return;
    }
    if (_next.text != _again.text) {
      setState(() => _error = "The two new passwords don't match.");
      return;
    }
    if (widget.needsRecovery && _code.text.trim().isEmpty) {
      setState(() => _error = 'Give your recovery code to keep your data.');
      return;
    }
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      final code = await widget.connection.changePassword(_current.text, _next.text, recoveryCode: widget.needsRecovery ? _code.text.trim() : null);
      if (mounted) Navigator.of(context).pop(PasswordChange(recoveryCode: code));
    } on PasswordException catch (e) {
      if (mounted) setState(() => _error = e.wrongPassword ? 'The ${widget.required ? 'provisional' : 'current'} password is wrong.' : e.message);
    } catch (e) {
      if (mounted) setState(() => _error = e.toString());
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  void dispose() {
    _current.dispose();
    _next.dispose();
    _again.dispose();
    _code.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return AlertDialog(
      title: Text(widget.required ? 'Choose your password' : 'Change your password'),
      content: SingleChildScrollView(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            if (widget.required) Text('Hi ${widget.name}. Before you start, replace the provisional password with one of your own.'),
            TextField(controller: _current, obscureText: true, decoration: InputDecoration(labelText: widget.required ? 'Provisional password' : 'Current password')),
            if (widget.needsRecovery)
              TextField(
                key: const Key('change-password-recovery-code'),
                controller: _code,
                autocorrect: false,
                textCapitalization: TextCapitalization.characters,
                decoration: const InputDecoration(
                  labelText: 'Recovery code',
                  helperText: 'Whoever runs the hub reset your password. Your data only comes back with the code you wrote down.',
                  helperMaxLines: 3,
                ),
              ),
            TextField(controller: _next, obscureText: true, decoration: const InputDecoration(labelText: 'New password')),
            TextField(controller: _again, obscureText: true, decoration: const InputDecoration(labelText: 'New password again')),
            if (_error != null) Padding(padding: const EdgeInsets.only(top: 12), child: Text(_error!, style: const TextStyle(color: Colors.red))),
          ],
        ),
      ),
      actions: [
        TextButton(onPressed: _busy ? null : () => Navigator.of(context).pop(), child: const Text('Cancel')),
        FilledButton(onPressed: _busy ? null : _submit, child: const Text('Save')),
      ],
    );
  }
}

/// The workspace's recovery policy changed to a weaker one (P84 fatia 4 parte B): their data follows it
/// only if they say yes, with their password. Pops the new recovery code (or an empty string when there
/// isn't one) once accepted, null for "Not now".
class AcceptPolicyDialog extends StatefulWidget {
  const AcceptPolicyDialog({super.key, required this.connection, required this.policy});

  final ServerConnection connection;

  /// The workspace's policy: `consent` or `company`.
  final String policy;

  @override
  State<AcceptPolicyDialog> createState() => _AcceptPolicyDialogState();
}

class _AcceptPolicyDialogState extends State<AcceptPolicyDialog> {
  final _password = TextEditingController();
  String? _error;
  bool _busy = false;

  Future<void> _accept() async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      final code = await widget.connection.acceptRecoveryPolicy(_password.text);
      if (mounted) Navigator.of(context).pop(code ?? '');
    } on PasswordException catch (e) {
      if (mounted) setState(() => _error = e.wrongPassword ? 'That password is wrong.' : e.message);
    } catch (e) {
      if (mounted) setState(() => _error = e.toString());
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  void dispose() {
    _password.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final company = widget.policy == 'company';
    return AlertDialog(
      title: const Text('Who can help recover your data changed'),
      content: SingleChildScrollView(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(company
                ? 'Whoever runs the hub can recover your data alone, with their recovery key. Every recovery is recorded and you are told.'
                : 'Whoever runs the hub can recover your data only together with your recovery code: neither opens it alone.'),
            const SizedBox(height: 8),
            const Text('Today your data follows the previous rule; it only follows this one if you accept.'),
            const SizedBox(height: 8),
            const Text('Keep in mind: whoever controls the machine the hub runs on can always, technically, see what the agent sees while it works. The record and the notice do not protect against someone editing the hub\'s files.'),
            TextField(key: const Key('accept-policy-password'), controller: _password, obscureText: true, onChanged: (_) => setState(() {}), decoration: const InputDecoration(labelText: 'Your password, to accept')),
            if (_error != null) Padding(padding: const EdgeInsets.only(top: 12), child: Text(_error!, style: const TextStyle(color: Colors.red))),
          ],
        ),
      ),
      actions: [
        TextButton(onPressed: _busy ? null : () => Navigator.of(context).pop(), child: const Text('Not now')),
        FilledButton(onPressed: _busy || _password.text.isEmpty ? null : _accept, child: const Text('Accept')),
      ],
    );
  }
}

/// The owner recovered their data (P84 fatia 4 parte B): they're told, once.
class RecoveryNoticeDialog extends StatelessWidget {
  const RecoveryNoticeDialog({super.key, required this.events});

  final List<RecoveryEvent> events;

  static String _when(int atMs) {
    final t = DateTime.fromMillisecondsSinceEpoch(atMs).toLocal();
    String two(int n) => n.toString().padLeft(2, '0');
    return '${t.year}-${two(t.month)}-${two(t.day)} ${two(t.hour)}:${two(t.minute)}';
  }

  @override
  Widget build(BuildContext context) {
    return AlertDialog(
      title: const Text('Your data was recovered'),
      content: SingleChildScrollView(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            const Text('Whoever runs the hub recovered your data, with their recovery key, and you signed in just now with the provisional password they gave you.'),
            const SizedBox(height: 8),
            for (final event in events) Text('${_when(event.atMs)} — ${event.kind == 'company' ? 'company recovery' : 'recovery with your consent'}'),
            const SizedBox(height: 8),
            const Text("If it wasn't you who asked, tell whoever runs the hub and change your password."),
          ],
        ),
      ),
      actions: [
        TextButton(onPressed: () => Navigator.of(context).pop(false), child: const Text('Not now')),
        FilledButton(onPressed: () => Navigator.of(context).pop(true), child: const Text('Got it')),
      ],
    );
  }
}

/// A member's own account: password, recovery code and TruthID. Pushed from the connection screen.
class AccountScreen extends StatefulWidget {
  const AccountScreen({super.key, required this.connection});

  final ServerConnection connection;

  @override
  State<AccountScreen> createState() => _AccountScreenState();
}

class _AccountScreenState extends State<AccountScreen> {
  String? _truthid;

  @override
  void initState() {
    super.initState();
    final user = widget.connection.user;
    _truthid = user != null && user.truthid.isNotEmpty ? user.truthid : null;
  }

  Future<void> _changePassword() async {
    final user = widget.connection.user;
    final change = await showDialog<PasswordChange>(
      context: context,
      builder: (_) => ChangePasswordDialog(connection: widget.connection, name: user?.name ?? '', required: false),
    );
    final code = change?.recoveryCode;
    if (code != null && mounted) await showRecoveryCode(context, code);
  }

  Future<void> _newCode() async {
    final password = await showDialog<String>(context: context, builder: (_) => const _AskPasswordDialog(title: 'New recovery code', hint: 'The old code stops working. Type your password to get a new one.'));
    if (password == null || !mounted) return;
    try {
      final code = await widget.connection.regenerateRecoveryCode(password);
      if (mounted) await showRecoveryCode(context, code, replacing: true);
    } on PasswordException catch (e) {
      if (mounted) ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(e.wrongPassword ? 'That password is wrong.' : e.message)));
    }
  }

  Future<void> _linkTruthId() async {
    final linked = await showDialog<String>(context: context, builder: (_) => _LinkTruthIdDialog(connection: widget.connection));
    if (linked != null && mounted) setState(() => _truthid = linked);
  }

  @override
  Widget build(BuildContext context) {
    final user = widget.connection.user;
    return Scaffold(
      appBar: AppBar(title: Text(user?.name ?? 'Account')),
      body: ListView(
        children: [
          ListTile(leading: const Icon(Icons.person), title: Text(user?.id ?? ''), subtitle: Text(user?.encrypted ?? false ? 'Your data is encrypted on the hub' : 'Your data is not encrypted yet — it is once you sign in with your password')),
          ListTile(key: const Key('account-change-password'), leading: const Icon(Icons.lock_reset), title: const Text('Change password'), onTap: _changePassword),
          if (user?.encrypted ?? false) ListTile(key: const Key('account-new-code'), leading: const Icon(Icons.key), title: const Text('Get a new recovery code'), subtitle: const Text('The old one stops working'), onTap: _newCode),
          ListTile(
            key: const Key('account-link-truthid'),
            leading: const Icon(Icons.verified_user),
            title: Text(_truthid == null ? 'Link my TruthID' : 'TruthID: @$_truthid'),
            subtitle: Text(_truthid == null ? 'With the invite code whoever runs the hub gave you' : 'To change it, ask whoever runs the hub for a new invite'),
            onTap: _truthid == null ? _linkTruthId : null,
          ),
        ],
      ),
    );
  }
}

class _AskPasswordDialog extends StatefulWidget {
  const _AskPasswordDialog({required this.title, required this.hint});

  final String title;
  final String hint;

  @override
  State<_AskPasswordDialog> createState() => _AskPasswordDialogState();
}

class _AskPasswordDialogState extends State<_AskPasswordDialog> {
  final _password = TextEditingController();

  @override
  void dispose() {
    _password.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return AlertDialog(
      title: Text(widget.title),
      content: Column(mainAxisSize: MainAxisSize.min, crossAxisAlignment: CrossAxisAlignment.start, children: [
        Text(widget.hint),
        TextField(controller: _password, obscureText: true, autofocus: true, decoration: const InputDecoration(labelText: 'Password')),
      ]),
      actions: [
        TextButton(onPressed: () => Navigator.of(context).pop(), child: const Text('Cancel')),
        FilledButton(onPressed: () => Navigator.of(context).pop(_password.text), child: const Text('Continue')),
      ],
    );
  }
}

class _LinkTruthIdDialog extends StatefulWidget {
  const _LinkTruthIdDialog({required this.connection});

  final ServerConnection connection;

  @override
  State<_LinkTruthIdDialog> createState() => _LinkTruthIdDialogState();
}

class _LinkTruthIdDialogState extends State<_LinkTruthIdDialog> {
  final _code = TextEditingController();
  final _username = TextEditingController();
  String? _error;
  bool _busy = false;

  Future<void> _link() async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      final linked = await widget.connection.redeemInvite(_code.text.trim(), _username.text.trim());
      if (mounted) Navigator.of(context).pop(linked);
    } on PasswordException catch (e) {
      if (mounted) setState(() => _error = e.message);
    } catch (e) {
      if (mounted) setState(() => _error = e.toString());
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  void dispose() {
    _code.dispose();
    _username.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return AlertDialog(
      title: const Text('Link my TruthID'),
      content: SingleChildScrollView(
        child: Column(mainAxisSize: MainAxisSize.min, children: [
          TextField(key: const Key('truthid-invite-code'), controller: _code, autocorrect: false, decoration: const InputDecoration(labelText: 'Invite code')),
          TextField(key: const Key('truthid-username'), controller: _username, autocorrect: false, decoration: const InputDecoration(labelText: 'My TruthID username', hintText: 'ana.silva')),
          if (_error != null) Padding(padding: const EdgeInsets.only(top: 12), child: Text(_error!, style: const TextStyle(color: Colors.red))),
        ]),
      ),
      actions: [
        TextButton(onPressed: _busy ? null : () => Navigator.of(context).pop(), child: const Text('Cancel')),
        FilledButton(onPressed: _busy ? null : _link, child: const Text('Link')),
      ],
    );
  }
}
