-- ************************************************************ 
-- Title:	PUBLIC_DVD_GENCONSETINVOKE_201908.ctl 
-- ************************************************************  
-- Project : NEMMCO MONTHLY DVD RELEASE 
-- Purpose : Control File to load monthly csv file 
-- Target Application  : Monthly DVD Release 
-- Application Version : 1 
-- Visibility Id : PUBLIC 
-- 
-- ************************************************************ 
-- Special Instructions : 1. USER TO CONNECT AS GENCONSETINVOKE owner 
-- 
-- ************************************************************ 
-- History : 
--      MMS Data Model Version   Template Generated On	
--      ======================   ======================	
--      PUBLIC			       v    
-- ************************************************************ 
LOAD DATA 
INFILE PUBLIC_DVD_GENCONSETINVOKE_201908010000.csv  
APPEND INTO TABLE GENCONSETINVOKE	
WHEN (1:1) = 'D'  
FIELDS TERMINATED BY ','  
OPTIONALLY ENCLOSED BY '"'  
TRAILING NULLCOLS	
(row_type FILLER, 
report_type FILLER, 
report_subtype FILLER, 
report_version FILLER, 
STARTDATE DATE "yyyy/mm/dd hh24:mi:ss", 
STARTPERIOD FLOAT EXTERNAL, 
GENCONSETID CHAR(20), 
ENDDATE DATE "yyyy/mm/dd hh24:mi:ss", 
ENDPERIOD FLOAT EXTERNAL, 
STARTAUTHORISEDBY CHAR(15), 
ENDAUTHORISEDBY CHAR(15), 
INTERVENTION CHAR(1), 
ASCONSTRAINTTYPE CHAR(10), 
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss", 
STARTINTERVALDATETIME DATE "yyyy/mm/dd hh24:mi:ss", 
ENDINTERVALDATETIME DATE "yyyy/mm/dd hh24:mi:ss", 
SYSTEMNORMAL CHAR(1), 
INVOCATION_ID FLOAT EXTERNAL) 
-- End of Script 
