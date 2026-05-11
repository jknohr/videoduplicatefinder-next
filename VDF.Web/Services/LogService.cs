// /*
//     Copyright (C) 2025 0x90d
//     This file is part of VideoDuplicateFinder
//     VideoDuplicateFinder is free software: you can redistribute it and/or modify
//     it under the terms of the GNU Affero General Public License as published by
//     the Free Software Foundation, either version 3 of the License, or
//     (at your option) any later version.
//     VideoDuplicateFinder is distributed in the hope that it will be useful,
//     but WITHOUT ANY WARRANTY without even the implied warranty of
//     MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
//     GNU Affero General Public License for more details.
//     You should have received a copy of the GNU Affero General Public License
//     along with VideoDuplicateFinder.  If not, see <http://www.gnu.org/licenses/>.
// */
//

using VDF.Core.Utils;

namespace VDF.Web.Services {
	/// <summary>
	/// Maintains a rolling in-memory buffer of log messages and notifies Blazor
	/// components whenever a new entry arrives.  Mirrors the ScanService pattern so
	/// UI components can subscribe to <see cref="StateChanged"/> and call
	/// <c>StateHasChanged()</c> on the Blazor renderer thread.
	/// </summary>
	public sealed class LogService {
		const int MaxEntries = 500;
		readonly Queue<string> _entries = new(MaxEntries + 1);
		readonly object _lock = new();

		/// <summary>Fired on the thread that produced the log message whenever a new entry arrives.</summary>
		public event Action? StateChanged;

		public LogService() {
			Logger.Instance.LogItemAdded += OnLogEntry;
		}

		/// <summary>Snapshot of recent log entries, oldest first.</summary>
		public IReadOnlyList<string> Entries {
			get {
				lock (_lock) return _entries.ToArray();
			}
		}

		public void Clear() {
			lock (_lock) _entries.Clear();
			StateChanged?.Invoke();
		}

		void OnLogEntry(string message) {
			lock (_lock) {
				if (_entries.Count >= MaxEntries) _entries.Dequeue();
				_entries.Enqueue(message);
			}
			StateChanged?.Invoke();
		}
	}
}
