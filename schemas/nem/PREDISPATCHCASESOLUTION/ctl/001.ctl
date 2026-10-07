-- ************************************************************ 
-- Title:	PUBLIC_DVD_PREDISPATCHCASESOLUTION_201501.ctl 
-- ************************************************************  
-- Project : NEMMCO MONTHLY DVD RELEASE 
-- Purpose : Control File to load monthly csv file 
-- Target Application  : Monthly DVD Release 
-- Application Version : 1 
-- Visibility Id : PUBLIC 
-- 
-- ************************************************************ 
-- Special Instructions : 1. USER TO CONNECT AS PREDISPATCHCASESOLUTION owner 
-- 
-- ************************************************************ 
-- History : 
--      MMS Data Model Version   Template Generated On	
--      ======================   ======================	
--      PUBLIC			       v4.23	
-- ************************************************************ 
LOAD DATA 
INFILE PUBLIC_DVD_PREDISPATCHCASESOLUTION_201501010000.csv  
APPEND INTO TABLE PREDISPATCHCASESOLUTION	
WHEN (1:1) = 'D'  
FIELDS TERMINATED BY ','  
OPTIONALLY ENCLOSED BY '"'  
TRAILING NULLCOLS	
(row_type FILLER, 
report_type FILLER, 
report_subtype FILLER, 
report_version FILLER, 
PREDISPATCHSEQNO CHAR(20), 
RUNNO FLOAT EXTERNAL, 
SOLUTIONSTATUS FLOAT EXTERNAL, 
SPDVERSION CHAR(20), 
NONPHYSICALLOSSES FLOAT EXTERNAL, 
TOTALOBJECTIVE FLOAT EXTERNAL, 
TOTALAREAGENVIOLATION FLOAT EXTERNAL, 
TOTALINTERCONNECTORVIOLATION FLOAT EXTERNAL, 
TOTALGENERICVIOLATION FLOAT EXTERNAL, 
TOTALRAMPRATEVIOLATION FLOAT EXTERNAL, 
TOTALUNITMWCAPACITYVIOLATION FLOAT EXTERNAL, 
TOTAL5MINVIOLATION FLOAT EXTERNAL, 
TOTALREGVIOLATION FLOAT EXTERNAL, 
TOTAL6SECVIOLATION FLOAT EXTERNAL, 
TOTAL60SECVIOLATION FLOAT EXTERNAL, 
TOTALASPROFILEVIOLATION FLOAT EXTERNAL, 
TOTALENERGYCONSTRVIOLATION FLOAT EXTERNAL, 
TOTALENERGYOFFERVIOLATION FLOAT EXTERNAL, 
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss", 
INTERVENTION FLOAT EXTERNAL) 
-- End of Script 
