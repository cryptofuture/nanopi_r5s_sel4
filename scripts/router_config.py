import ipaddress,json

def validate(config):
 expected={'version': 1, 'wan': {'device': 'gmac0', 'mac': '02:52:35:53:00:01', 'configuration': 'dhcp'}, 'lan1': {'device': 'rtl8125-apb-fe260000', 'mac': '02:52:35:53:00:02', 'address': '10.44.1.1', 'prefix': 24, 'pool': [100, 199]}, 'lan2': {'device': 'rtl8125-apb-fe270000', 'mac': '02:52:35:53:00:03', 'address': '10.44.2.1', 'prefix': 24, 'pool': [100, 199]}, 'dns_override': [], 'policy': 'outbound-stateful-no-inter-lan', 'fragments': 'drop-laboratory-only'}
 if type(config.get('version')) is not int:raise ValueError('Version must be an integer')
 dns=config.get('dns_override')
 if not isinstance(dns,list) or len(dns)>3:raise ValueError('DNS override limit')
 for value in dns:
  if not isinstance(value,str):raise ValueError('DNS override must be an address string')
  ip=ipaddress.IPv4Address(value)
  if int(ip)>>24==0 or ip.is_unspecified or ip.is_loopback or ip.is_multicast or int(ip)>=0xe0000000 or ip in ipaddress.IPv4Network('10.44.1.0/24') or ip in ipaddress.IPv4Network('10.44.2.0/24'):raise ValueError('Unusable DNS override')
 expected['dns_override']=dns
 if config!=expected:raise ValueError('Unsupported router configuration')
 return config

def dns_source(config):
 values=[int(ipaddress.IPv4Address(ip)) for ip in validate(config)['dns_override']]
 return 'pub const DNS:[u32;3]=['+','.join(map(str,values+[0]*(3-len(values))))+'];\npub const DNS_COUNT:usize='+str(len(values))+';\n'
