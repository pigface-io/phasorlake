-- ************************************************************ 
-- Title:	PUBLIC_DVD_BIDPEROFFER_D_201501.ctl 
-- ************************************************************  
-- Project : NEMMCO MONTHLY DVD RELEASE 
-- Purpose : Control File to load monthly csv file 
-- Target Application  : Monthly DVD Release 
-- Application Version : 1 
-- Visibility Id : PUBLIC 
-- 
-- ************************************************************ 
-- Special Instructions : 1. USER TO CONNECT AS BIDPEROFFER_D owner 
-- 
-- ************************************************************ 
-- History : 
--      MMS Data Model Version   Template Generated On	
--      ======================   ======================	
--      PUBLIC			       v4.23	
-- ************************************************************ 
LOAD DATA 
INFILE PUBLIC_DVD_BIDPEROFFER_D_201501010000.csv  
APPEND INTO TABLE BIDPEROFFER_D  
WHEN (1:1) = 'D'  
FIELDS TERMINATED BY ','  
OPTIONALLY ENCLOSED BY '"'  
TRAILING NULLCOLS	
(row_type FILLER, 
report_type FILLER, 
report_subtype FILLER, 
report_version FILLER, 
SETTLEMENTDATE DATE "yyyy/mm/dd hh24:mi:ss", 
DUID CHAR(10), 
BIDTYPE CHAR(10), 
BIDSETTLEMENTDATE DATE "yyyy/mm/dd hh24:mi:ss", 
OFFERDATE DATE "yyyy/mm/dd hh24:mi:ss", 
PERIODID FLOAT EXTERNAL, 
VERSIONNO FLOAT EXTERNAL, 
MAXAVAIL FLOAT EXTERNAL, 
FIXEDLOAD FLOAT EXTERNAL, 
ROCUP FLOAT EXTERNAL, 
ROCDOWN FLOAT EXTERNAL, 
ENABLEMENTMIN FLOAT EXTERNAL, 
ENABLEMENTMAX FLOAT EXTERNAL, 
LOWBREAKPOINT FLOAT EXTERNAL, 
HIGHBREAKPOINT FLOAT EXTERNAL, 
BANDAVAIL1 FLOAT EXTERNAL, 
BANDAVAIL2 FLOAT EXTERNAL, 
BANDAVAIL3 FLOAT EXTERNAL, 
BANDAVAIL4 FLOAT EXTERNAL, 
BANDAVAIL5 FLOAT EXTERNAL, 
BANDAVAIL6 FLOAT EXTERNAL, 
BANDAVAIL7 FLOAT EXTERNAL, 
BANDAVAIL8 FLOAT EXTERNAL, 
BANDAVAIL9 FLOAT EXTERNAL, 
BANDAVAIL10 FLOAT EXTERNAL, 
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss", 
PASAAVAILABILITY FLOAT EXTERNAL, 
INTERVAL_DATETIME DATE "yyyy/mm/dd hh24:mi:ss", 
MR_CAPACITY FLOAT EXTERNAL) 
-- End of Script 
